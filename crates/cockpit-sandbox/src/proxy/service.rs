//! Explicit systemd/launchd commands and the configuration of the pinned proxy.
use std::collections::BTreeMap;
use std::process::Command;
use std::time::{Duration, Instant};

use cockpit_config::Profile;

use super::{ManagedProxy, ProxyError, command};

#[derive(Debug, Clone, Copy)]
pub(super) enum ServiceManager {
    Systemd,
    Launchd,
}

impl ServiceManager {
    pub(super) fn program(self) -> &'static str {
        match self {
            Self::Systemd => "systemctl",
            Self::Launchd => "launchctl",
        }
    }

    pub(super) fn unit_name(self, profile: &Profile) -> String {
        match self {
            Self::Systemd => format!("cockpit-greyproxy-{profile}.service"),
            Self::Launchd => format!("io.github.androlax2.cockpit.greyproxy.{profile}.plist"),
        }
    }
}

impl ManagedProxy {
    fn launchd_target(&self) -> String {
        format!(
            "gui/{}/io.github.androlax2.cockpit.greyproxy.{}",
            self.uid, self.profile
        )
    }

    fn service_command(&self, arguments: &[&str]) -> Result<std::process::Output, ProxyError> {
        command::run(
            Command::new(&self.service_program)
                .env("HOME", &self.home)
                .env("COCKPIT_PROFILE", self.profile.name())
                .env("LC_ALL", "C")
                .args(arguments),
            None,
            self.command_deadline,
        )
        .map_err(ProxyError::ServiceManager)
    }

    fn service_action(&self, arguments: &[&str]) -> Result<(), ProxyError> {
        let output = self.service_command(arguments)?;
        if !output.status.success() {
            // Service output can include inherited environment values; keep it out of reports.
            return Err(ProxyError::ServiceManager(format!(
                "{} {} failed ({})",
                self.manager.program(),
                arguments.join(" "),
                output.status
            )));
        }
        Ok(())
    }

    pub(super) fn service_pid(&self) -> Result<Option<u32>, ProxyError> {
        self.service_pid_for_action(false)
    }

    /// Only explicit lifecycle actions may repair a stale, verified owned fragment.
    /// Readiness never changes the service manager's state.
    pub(super) fn service_pid_for_action(
        &self,
        reload_owned: bool,
    ) -> Result<Option<u32>, ProxyError> {
        let output = match self.manager {
            ServiceManager::Systemd => self.service_command(&[
                "--user",
                "show",
                &self.manager.unit_name(&self.profile),
                "--property=ActiveState,SubState,MainPID,FragmentPath,NeedDaemonReload,DropInPaths",
            ])?,
            ServiceManager::Launchd => self.service_command(&["print", &self.launchd_target()])?,
        };
        if !output.status.success() {
            if matches!(self.manager, ServiceManager::Launchd) {
                // `print` of an unloaded job fails; distinguish that from an unavailable
                // login domain before calling the service stopped.
                self.service_action(&["print", &format!("gui/{}", self.uid)])?;
                return Ok(None);
            }
            return Err(ProxyError::ServiceManager(format!(
                "{} could not inspect this profile's service ({})",
                self.manager.program(),
                output.status
            )));
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let fields = service_fields(&text);
        let unit = self.unit_path.to_string_lossy();
        let program = self.program.to_string_lossy();
        let pid = |key| {
            fields
                .get(key)
                .and_then(|value| value.parse::<u32>().ok())
                .filter(|value| *value > 0)
        };
        let pid = match self.manager {
            ServiceManager::Systemd => {
                if fields.get("FragmentPath") != Some(&unit.as_ref())
                    || fields.get("DropInPaths") != Some(&"")
                {
                    return Err(ProxyError::Unhealthy(
                        "systemd is not running the current owned unit".into(),
                    ));
                }
                if fields.get("NeedDaemonReload") != Some(&"no") {
                    if reload_owned && fields.get("NeedDaemonReload") == Some(&"yes") {
                        self.service_action(&["--user", "daemon-reload"])?;
                        return self.service_pid();
                    }
                    return Err(ProxyError::Unhealthy(
                        "systemd needs to reload the owned unit; run sandbox restart".into(),
                    ));
                }
                if fields.get("ActiveState") != Some(&"active") {
                    return Ok(None);
                }
                if fields.get("SubState") != Some(&"running") {
                    return Err(ProxyError::Unhealthy(
                        "systemd has not reached the running state".into(),
                    ));
                }
                pid("MainPID")
            }
            ServiceManager::Launchd => {
                if fields.get("path") != Some(&unit.as_ref())
                    || fields.get("program") != Some(&program.as_ref())
                {
                    return Err(ProxyError::Unhealthy(
                        "launchd is not running the current owned engine and unit".into(),
                    ));
                }
                let arguments = launchd_block(&text, "arguments");
                let expected = [
                    program.as_ref(),
                    "serve",
                    "-C",
                    &path_text(&self.configuration_path())?,
                    "--require-api-token",
                    "--require-client-secret",
                ];
                if arguments != expected
                    || !launchd_block(&text, "environment").iter().any(|line| {
                        line.split_once("=>").is_some_and(|(name, value)| {
                            name.trim() == "GREYPROXY_DATA_HOME"
                                && value.trim() == self.data_dir.to_string_lossy()
                        })
                    })
                {
                    return Err(ProxyError::Unhealthy("launchd's loaded arguments or data directory differ from the owned configuration".into()));
                }
                if fields.get("state") != Some(&"running") {
                    return Ok(None);
                }
                pid("pid")
            }
        };
        Ok(pid)
    }

    pub(super) fn start_service(&self) -> Result<(), ProxyError> {
        match self.manager {
            ServiceManager::Systemd => {
                self.service_action(&["--user", "daemon-reload"])?;
                self.service_action(&["--user", "enable", &path_text(&self.unit_path)?])?;
                self.service_action(&["--user", "start", &self.manager.unit_name(&self.profile)])
            }
            ServiceManager::Launchd => {
                self.service_action(&["enable", &self.launchd_target()])?;
                if self
                    .service_command(&["print", &self.launchd_target()])?
                    .status
                    .success()
                {
                    self.service_pid()?; // Validate even a loaded, stopped job before kickstart.
                    return self.service_action(&["kickstart", &self.launchd_target()]);
                }
                self.service_action(&[
                    "bootstrap",
                    &format!("gui/{}", self.uid),
                    &path_text(&self.unit_path)?,
                ])
            }
        }
    }

    pub(super) fn stop_service(&self) -> Result<(), ProxyError> {
        if let Some(pid) = self.service_pid_for_action(true)? {
            self.verify_service_process(pid)?;
        }
        match self.manager {
            ServiceManager::Systemd => {
                self.service_action(&["--user", "stop", &self.manager.unit_name(&self.profile)])
            }
            ServiceManager::Launchd => {
                if let Err(error) = self.service_action(&["bootout", &self.launchd_target()]) {
                    let loaded = self.service_command(&["print", &self.launchd_target()])?;
                    if loaded.status.success() {
                        return Err(error);
                    }
                    self.service_action(&["print", &format!("gui/{}", self.uid)])?;
                }
                // bootout is asynchronous: do not race the old process for its ports or
                // bootstrap a label launchd has not removed yet.
                let deadline = Instant::now() + super::STARTUP_DEADLINE;
                loop {
                    let loaded = self.service_command(&["print", &self.launchd_target()])?;
                    if !loaded.status.success() {
                        return self.service_action(&["print", &format!("gui/{}", self.uid)]);
                    }
                    if Instant::now() >= deadline {
                        return Err(ProxyError::ServiceManager(
                            "launchd did not unload the owned job within the stop deadline".into(),
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }
    }

    pub(super) fn port_map(&self) -> serde_json::Value {
        serde_json::json!({
            "api": self.ports[0], "http-proxy": self.ports[1], "socks5-proxy": self.ports[2],
            "dns-proxy-udp": self.ports[3], "dns-proxy-tcp": self.ports[3],
        })
    }

    pub(super) fn configuration(&self) -> String {
        let traffic = |name: &str, port, kind: &str| {
            serde_json::json!({
                "name": name, "addr": format!("127.0.0.1:{port}"),
                "handler": {"type":kind, "auther":"auther-0", "metadata":{
                    "sniffing":true,"sniffing.websocket":true,
                    "mitm.certFile":self.data_dir.join("ca-cert.pem"),
                    "mitm.keyFile":self.data_dir.join("ca-key.pem"),
                }},
                "listener":{"type":"tcp"}, "admission":"admission-0", "bypass":"bypass-0",
                "resolver":"resolver-0", "hosts":"hosts-0",
            })
        };
        let dns = |name: &str, listener: &str| {
            serde_json::json!({
                "name":name,"addr":format!("127.0.0.1:{}", self.ports[3]),
                "handler":{"type":"dns"}, "listener":{"type":listener,"metadata":{"mode":"udp"}},
                "resolver":"resolver-0",
            })
        };
        format!(
            "{}\n",
            serde_json::json!({
                "host":"127.0.0.1", "log":{"level":"error","format":"json","output":"stdout"},
                "greyproxy":{
                    "addr":format!("127.0.0.1:{}",self.ports[0]), "pathPrefix":"/",
                    "auther":"auther-0","admission":"admission-0","bypass":"bypass-0","resolver":"resolver-0",
                    "requireApiToken":true,"requireClientSecret":true,"clientSecretExempt":[],"allowAnonymousClients":false,
                    "notifications":{"enabled":false}, "docker":{"enabled":false},
                },
                "services":[traffic("http-proxy",self.ports[1],"http"),traffic("socks5-proxy",self.ports[2],"socks5"),
                    dns("dns-proxy-udp","dns"),dns("dns-proxy-tcp","tcp")],
                "hosts":[{"name":"hosts-0","mappings":[{"ip":"127.0.0.1","hostname":"localhost"},{"ip":"::1","hostname":"localhost"}]}],
            })
        )
    }

    pub(super) fn unit_text(&self) -> Result<String, ProxyError> {
        let program = path_text(&self.program)?;
        let configuration = path_text(&self.configuration_path())?;
        let data = path_text(&self.data_dir)?;
        let home = path_text(&self.home)?;
        let marker = format!("Cockpit managed Greyproxy, profile {}", self.profile);
        Ok(match self.manager {
            ServiceManager::Systemd => format!(
                "# {marker}\n[Unit]\nDescription={marker}\n\n[Service]\nType=simple\nUMask=0077\n\
                 Environment={}\nEnvironment={}\nEnvironment=GREYPROXY_DOCKER_ENABLED=false\n\
                 ExecStart=:{} serve -C {} --require-api-token --require-client-secret\n\
                 Restart=on-failure\nRestartSec=2\nTimeoutStopSec=5\n\n[Install]\nWantedBy=default.target\n",
                systemd_quote(&format!("HOME={home}")),
                systemd_quote(&format!("GREYPROXY_DATA_HOME={data}")),
                systemd_quote(&program),
                systemd_quote(&configuration),
            ),
            ServiceManager::Launchd => {
                let label = format!("io.github.androlax2.cockpit.greyproxy.{}", self.profile);
                let args = [
                    program.as_str(),
                    "serve",
                    "-C",
                    configuration.as_str(),
                    "--require-api-token",
                    "--require-client-secret",
                ]
                .into_iter()
                .map(|value| format!("<string>{}</string>", xml(value)))
                .collect::<String>();
                format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
                    <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
                    <!-- {marker} -->\n<plist version=\"1.0\"><dict>\n\
                    <key>Label</key><string>{label}</string>\n<key>ProgramArguments</key><array>{args}</array>\n\
                    <key>EnvironmentVariables</key><dict>\n<key>HOME</key><string>{}</string>\n\
                    <key>GREYPROXY_DATA_HOME</key><string>{}</string>\n\
                    <key>GREYPROXY_DOCKER_ENABLED</key><string>false</string></dict>\n\
                    <key>Umask</key><integer>63</integer>\n<key>RunAtLoad</key><true/>\n\
                    <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>\n\
                    <key>ThrottleInterval</key><integer>2</integer>\n</dict></plist>\n",
                    xml(&home),
                    xml(&data)
                )
            }
        })
    }
}

fn path_text(path: &std::path::Path) -> Result<String, ProxyError> {
    path.to_str()
        .filter(|text| !text.chars().any(char::is_control))
        .map(str::to_owned)
        .ok_or_else(|| ProxyError::UnsafePath(path.into()))
}

fn systemd_quote(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
    )
}

fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn launchd_block<'a>(text: &'a str, name: &str) -> Vec<&'a str> {
    let start = format!("{name} = {{");
    text.lines()
        .skip_while(|line| line.trim() != start)
        .skip(1)
        .map(str::trim)
        .take_while(|line| *line != "}")
        .collect()
}

fn service_fields(text: &str) -> BTreeMap<&str, &str> {
    let root = usize::from(
        text.lines()
            .next()
            .is_some_and(|line| line.trim_end().ends_with("= {")),
    );
    let mut depth = 0usize;
    let mut fields = BTreeMap::new();
    for line in text.lines().map(str::trim) {
        if line.ends_with("= {") {
            depth += 1;
            continue;
        }
        if line == "}" {
            depth = depth.saturating_sub(1);
            continue;
        }
        if depth == root
            && let Some((name, value)) = line.split_once('=')
        {
            fields.insert(name.trim(), value.trim());
        }
    }
    fields
}
