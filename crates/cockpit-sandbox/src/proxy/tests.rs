use super::*;
use std::os::unix::fs::{PermissionsExt, symlink};

struct Fixture {
    _home: tempfile::TempDir,
    proxy: ManagedProxy,
}

fn executable(path: &Path, text: &str) {
    fs::write(path, format!("#!/bin/sh\n{text}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

impl Fixture {
    fn new(platform: Platform) -> Self {
        let home = tempfile::tempdir().unwrap();
        let environment = cockpit_config::Environment {
            home: Some(home.path().into()),
            ..cockpit_config::Environment::default()
        };
        let mut proxy = ManagedProxy::new(&environment, &platform).unwrap();
        proxy.service_program = home.path().join("manager");
        proxy.curl = home.path().join("curl");
        proxy.proc_root = home.path().join("proc");
        proxy.lsof = home.path().join("lsof");
        proxy.metadata_commands = home.path().join("metadata-tools");
        fs::create_dir(&proxy.metadata_commands).unwrap();
        for (name, real) in [
            ("cat", "/bin/cat"),
            ("ls", "/bin/ls"),
            ("readlink", "/usr/bin/readlink"),
        ] {
            executable(
                &proxy.metadata_commands.join(name),
                &format!("exec {real} \"$@\""),
            );
        }
        let listeners: Vec<_> = (0..4)
            .map(|_| std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap())
            .collect();
        proxy.ports = std::array::from_fn(|i| listeners[i].local_addr().unwrap().port());
        let fixture = Self { _home: home, proxy };
        fs::create_dir_all(fixture.proxy.program.parent().unwrap()).unwrap();
        executable(
            &fixture.proxy.program,
            &format!(
                "case \"$*\" in '--version') printf 'greyproxy 0.4.5 (fixture)\\n  commit: {}\\n';; 'cert generate') umask 077; printf '%s\\n' '-----BEGIN CERTIFICATE-----' 'fixture' '-----END CERTIFICATE-----' > \"$GREYPROXY_DATA_HOME/ca-cert.pem\"; printf '%s\\n' '-----BEGIN EC PRIVATE KEY-----' 'fixture' '-----END EC PRIVATE KEY-----' > \"$GREYPROXY_DATA_HOME/ca-key.pem\";; *) exit 99;; esac",
                crate::PINNED_GREYPROXY_COMMIT
            ),
        );
        let process = fixture.proxy.proc_root.join("123");
        fs::create_dir_all(process.join("fd")).unwrap();
        fs::create_dir_all(process.join("net")).unwrap();
        symlink(
            fixture.proxy.program.canonicalize().unwrap(),
            process.join("exe"),
        )
        .unwrap();
        fs::write(
            process.join("cmdline"),
            format!(
                "{}\0serve\0-C\0{}\0--require-api-token\0--require-client-secret\0",
                fixture.proxy.program.display(),
                fixture.proxy.configuration_path().display()
            ),
        )
        .unwrap();
        fs::write(
            process.join("environ"),
            format!("GREYPROXY_DATA_HOME={}\0", fixture.proxy.data_dir.display()),
        )
        .unwrap();
        for inode in 100..105 {
            symlink(
                format!("socket:[{inode}]"),
                process.join("fd").join(inode.to_string()),
            )
            .unwrap();
        }
        let tcp = fixture
            .proxy
            .ports
            .iter()
            .enumerate()
            .map(|(index, port)| {
                format!(
                    "{index}: 0100007F:{port:04X} 00000000:0000 0A 0 0 0 1000 0 {}\n",
                    100 + index
                )
            })
            .collect::<String>();
        fs::write(process.join("net/tcp"), format!("header\n{tcp}")).unwrap();
        fs::write(
            process.join("net/udp"),
            format!(
                "header\n0: 0100007F:{:04X} 00000000:0000 07 0 0 0 1000 0 104\n",
                fixture.proxy.ports[3]
            ),
        )
        .unwrap();
        let endpoints = fixture
            .proxy
            .ports
            .iter()
            .map(|port| format!("n127.0.0.1:{port}\n"))
            .collect::<String>();
        executable(&fixture.proxy.lsof, &format!("printf 'p123\n{endpoints}'"));
        fixture.manager(true);
        fixture.http("ok");
        fixture
    }

    fn manager(&self, active: bool) {
        let inactive = self
            .manager_body()
            .replace("ActiveState=active", "ActiveState=inactive")
            .replace("state = running", "state = waiting")
            .replace("MainPID=123", "MainPID=0")
            .replace("pid = 123", "pid = 0");
        let body = if active {
            self.manager_body()
        } else {
            inactive.clone()
        };
        executable(
            &self.proxy.service_program,
            &format!(
                "printf '%s\\n' \"$*\" >> \"$HOME/manager.log\"\ncase \"$*\" in *show*|*print*) if [ -e \"$HOME/manager.stopped\" ]; then case \"$*\" in *cockpit.greyproxy*) exit 113;; *show*) printf '%s' '{inactive}'; exit 0;; esac; fi; printf '%s' '{body}';; *stop*|*bootout*) /bin/touch \"$HOME/manager.stopped\";; *start*|*bootstrap*) /bin/rm -f \"$HOME/manager.stopped\"; umask 077; printf '%s' '{}' > '{}';; esac",
                "a".repeat(64),
                self.proxy.data_dir.join("api-token").display(),
            ),
        );
    }

    fn manager_body(&self) -> String {
        match self.proxy.manager {
            ServiceManager::Systemd => format!(
                "ActiveState=active\nSubState=running\nMainPID=123\nNeedDaemonReload=no\nDropInPaths=\nFragmentPath={}\n",
                self.proxy.unit_path.display()
            ),
            ServiceManager::Launchd => format!(
                "state = running\npid = 123\npath = {}\nprogram = {}\narguments = {{\n{}\nserve\n-C\n{}\n--require-api-token\n--require-client-secret\n}}\nenvironment = {{\nGREYPROXY_DATA_HOME => {}\n}}\n",
                self.proxy.unit_path.display(),
                self.proxy.program.display(),
                self.proxy.program.display(),
                self.proxy.configuration_path().display(),
                self.proxy.data_dir.display()
            ),
        }
    }

    fn http(&self, status: &str) {
        let health = serde_json::json!({
            "service": "greyproxy", "version": "0.4.5", "status": status,
            "ports": self.proxy.port_map(),
        });
        executable(
            &self.proxy.curl,
            &format!(
                "printf '%s\\n' \"$*\" >> \"$HOME/curl.log\"\ncase \"$*\" in *'http://[::1]:'*) exit 7;; esac\ninput=$(/bin/cat)\ncase \"$*\" in */api/health*) printf '%s\\n200' '{health}';; *) if [ -n \"$input\" ]; then printf '{{\"theme\":\"system\"}}\\n200'; else printf '{{}}\\n401'; fi;; esac"
            ),
        );
    }

    fn configure(&self) {
        self.proxy.write_configuration().unwrap();
        write_private(&self.proxy.data_dir.join("api-token"), &"a".repeat(64)).unwrap();
    }
}

#[test]
fn managed_units_and_configuration_preserve_authentication_and_profile_isolation() {
    for platform in [
        Platform::Linux,
        Platform::MacOs,
        Platform::Wsl2 {
            windows_drives: vec!["/mnt/c".into()],
            interop_sockets: "/run/WSL".into(),
        },
    ] {
        let fixture = Fixture::new(platform);
        fixture.configure();
        let config: serde_json::Value =
            serde_json::from_str(&fixture.proxy.configuration()).unwrap();
        assert_eq!(config["host"], "127.0.0.1");
        assert_eq!(config["greyproxy"]["requireApiToken"], true);
        assert_eq!(config["greyproxy"]["requireClientSecret"], true);
        assert_eq!(config["greyproxy"]["allowAnonymousClients"], false);
        assert_eq!(
            config["greyproxy"]["clientSecretExempt"],
            serde_json::json!([])
        );
        let unit = fs::read_to_string(fixture.proxy.unit_path()).unwrap();
        assert!(unit.contains("GREYPROXY_DATA_HOME"));
        assert!(unit.contains(&fixture.proxy.program.display().to_string()));
        assert!(!unit.contains("X-Cockpit-Profile="));
        assert_eq!(
            fs::metadata(fixture.proxy.data_dir())
                .unwrap()
                .permissions()
                .mode()
                & 0o077,
            0
        );
        assert!(fixture.proxy.health().is_ok());
    }
}

#[test]
fn matching_binary_and_public_health_are_insufficient_without_an_owned_running_service() {
    let fixture = Fixture::new(Platform::Linux);
    fixture.configure();
    fixture.manager(false);
    assert_eq!(
        fixture.proxy.health().unwrap_err().code(),
        "proxy-service-stopped"
    );
    assert!(!fixture.proxy.home.join("curl.log").exists());
    fixture.manager(true);
    fixture.http("unhealthy");
    assert_eq!(
        fixture.proxy.health().unwrap_err().code(),
        "proxy-unhealthy"
    );
}

#[test]
fn changed_units_and_symlinked_tokens_are_refused_before_sending_credentials() {
    let fixture = Fixture::new(Platform::Linux);
    fixture.configure();
    fs::write(fixture.proxy.unit_path(), "foreign service").unwrap();
    assert_eq!(
        fixture.proxy.stop().unwrap_err().code(),
        "proxy-configuration-changed"
    );
    assert!(!fixture.proxy.home.join("manager.log").exists());
    fs::write(
        fixture.proxy.unit_path(),
        fixture.proxy.unit_text().unwrap(),
    )
    .unwrap();
    let token = fixture.proxy.data_dir.join("api-token");
    fs::remove_file(&token).unwrap();
    symlink(fixture.proxy.home.join("foreign-secret"), &token).unwrap();
    assert_eq!(
        fixture.proxy.health().unwrap_err().code(),
        "proxy-unsafe-path"
    );
    assert!(!fixture.proxy.home.join("curl.log").exists());
}

#[test]
fn authenticated_health_never_puts_the_private_token_in_arguments() {
    let fixture = Fixture::new(Platform::Linux);
    fixture.configure();
    fixture.proxy.health().unwrap();
    let arguments = fs::read_to_string(fixture.proxy.home.join("curl.log")).unwrap();
    assert!(!arguments.contains(&"a".repeat(64)));
    assert!(arguments.contains("--noproxy *"));
    assert!(arguments.contains("--header @-"));
}

#[test]
fn a_public_management_api_is_unhealthy_even_when_the_health_body_matches() {
    let fixture = Fixture::new(Platform::Linux);
    fixture.configure();
    let health =
        serde_json::json!({"service":"greyproxy", "status":"ok", "ports":fixture.proxy.port_map()});
    executable(
        &fixture.proxy.curl,
        &format!("printf '%s\\n200' '{health}'"),
    );
    assert_eq!(
        fixture.proxy.health().unwrap_err().code(),
        "proxy-unhealthy"
    );
}

#[test]
fn occupied_tcp_and_udp_endpoints_are_refused_without_service_actions() {
    let fixture = Fixture::new(Platform::Linux);
    let listener =
        std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, fixture.proxy.ports[1]))
            .unwrap();
    assert_eq!(
        fixture.proxy.check_ports().unwrap_err().code(),
        "proxy-endpoint-occupied"
    );
    drop(listener);
    let _socket =
        std::net::UdpSocket::bind((std::net::Ipv4Addr::LOCALHOST, fixture.proxy.ports[3])).unwrap();
    assert_eq!(
        fixture.proxy.check_ports().unwrap_err().code(),
        "proxy-endpoint-occupied"
    );
    assert!(!fixture.proxy.home.join("manager.log").exists());
}

#[test]
fn a_hanging_service_command_has_a_deadline() {
    let mut fixture = Fixture::new(Platform::Linux);
    fixture.configure();
    fixture.proxy.command_deadline = Duration::from_millis(80);
    executable(&fixture.proxy.service_program, "/bin/sleep 1");
    let started = std::time::Instant::now();
    assert_eq!(
        fixture.proxy.health().unwrap_err().code(),
        "proxy-service-manager"
    );
    assert!(started.elapsed() < Duration::from_millis(700));
}

#[test]
fn installation_starts_an_owned_service_and_a_second_install_keeps_it_running() {
    for platform in [Platform::Linux, Platform::MacOs] {
        let fixture = Fixture::new(platform);
        fixture.proxy.install().unwrap();
        let log = fs::read_to_string(fixture.proxy.home.join("manager.log")).unwrap();
        fixture.proxy.install().unwrap();
        let repeated = fs::read_to_string(fixture.proxy.home.join("manager.log")).unwrap();
        assert!(!repeated[log.len()..].contains("start"), "{repeated}");
        assert!(!repeated[log.len()..].contains("bootstrap"), "{repeated}");
        fixture.proxy.stop().unwrap();
        let stopped = fs::read_to_string(fixture.proxy.home.join("manager.log")).unwrap();
        assert!(
            stopped.contains("stop") || stopped.contains("bootout"),
            "{stopped}"
        );
    }
}

#[test]
fn fresh_install_initializes_the_private_ca_before_starting_the_service() {
    let fixture = Fixture::new(Platform::Linux);
    fixture.proxy.install().unwrap();
    for name in ["ca-cert.pem", "ca-key.pem"] {
        let path = fixture.proxy.data_dir.join(name);
        assert!(path.is_file(), "missing {name}");
        assert_eq!(fs::metadata(path).unwrap().mode() & 0o077, 0);
    }
}

#[test]
fn missing_ca_state_refuses_health_before_sending_an_api_token() {
    let fixture = Fixture::new(Platform::Linux);
    fixture.configure();
    let key = fixture.proxy.data_dir.join("ca-key.pem");
    if key.exists() {
        fs::remove_file(key).unwrap();
    }
    assert!(fixture.proxy.health().is_err());
    assert!(!fixture.proxy.home.join("curl.log").exists());
}

#[test]
fn macos_launches_fail_closed_until_the_engine_can_expose_only_the_public_ca() {
    let fixture = Fixture::new(Platform::MacOs);
    fixture.configure();
    fixture.proxy.health().unwrap();
    let error = fixture.proxy.launch_health(None).unwrap_err();
    assert_eq!(error.code(), "proxy-engine-capability");
    assert!(error.to_string().contains(crate::PINNED_GREYWALL_COMMIT));
    assert!(
        error
            .next(&fixture.proxy.profile)
            .contains("verified Greywall release")
    );
}

#[test]
fn a_partial_ca_generation_is_cleaned_up_and_does_not_start_a_service() {
    let fixture = Fixture::new(Platform::Linux);
    let original = fs::read_to_string(&fixture.proxy.program).unwrap();
    executable(
        &fixture.proxy.program,
        &format!(
            "case \"$*\" in 'cert generate') printf partial > \"$GREYPROXY_DATA_HOME/ca-cert.pem\"; exit 1;; esac\n{original}"
        ),
    );
    assert!(fixture.proxy.install().is_err());
    assert!(!fixture.proxy.home.join("manager.log").exists());
    assert_eq!(fs::read_dir(&fixture.proxy.data_dir).unwrap().count(), 1); // setup.lock only
    fs::write(&fixture.proxy.program, original).unwrap();
    fixture.proxy.install().unwrap();
}

#[test]
fn unrelated_namespace_sockets_do_not_hide_the_owned_listeners() {
    let fixture = Fixture::new(Platform::Linux);
    fixture.configure();
    let path = fixture.proxy.proc_root.join("123/net/tcp");
    let owned = fs::read_to_string(&path).unwrap();
    // A static table fixture, not a socket/load test. These TIME_WAIT rows belong
    // to unrelated applications in the same network namespace.
    let unrelated =
        "0: 0100007F:0001 0100007F:0002 06 00000000:00000000 03:00000000 00000000 1000 0 999\n"
            .repeat(1000);
    assert!(unrelated.len() > 64 * 1024);
    fs::write(path, format!("header\n{unrelated}{owned}")).unwrap();
    fixture.proxy.health().unwrap();
}

#[test]
fn explicit_setup_retries_an_owned_systemd_unit_needing_reload() {
    for restart in [false, true] {
        let fixture = Fixture::new(Platform::Linux);
        fixture.configure();
        let body = fixture.manager_body();
        let stale = body.replace("NeedDaemonReload=no", "NeedDaemonReload=yes");
        fs::write(fixture.proxy.home.join("needs-reload"), "").unwrap();
        fs::write(fixture.proxy.home.join("reload-fails-once"), "").unwrap();
        executable(
            &fixture.proxy.service_program,
            &format!(
                r#"
printf '%s\n' "$*" >> "$HOME/manager.log"
case "$*" in
*daemon-reload*) if [ -e "$HOME/reload-fails-once" ]; then /bin/rm "$HOME/reload-fails-once"; exit 1; fi; /bin/rm -f "$HOME/needs-reload";;
*show*) if [ -e "$HOME/needs-reload" ]; then printf '%s' '{stale}'; else printf '%s' '{body}'; fi;;
esac
"#
            ),
        );
        assert!(fixture.proxy.health().is_err());
        let first = if restart {
            fixture.proxy.restart()
        } else {
            fixture.proxy.install()
        };
        assert_eq!(first.unwrap_err().code(), "proxy-service-manager");
        if restart {
            fixture.proxy.restart().unwrap();
        } else {
            fixture.proxy.install().unwrap();
        }
        fixture.proxy.health().unwrap();
        assert!(!fixture.proxy.home.join("needs-reload").exists());
    }
}

#[test]
fn install_refuses_existing_unowned_state_instead_of_adopting_its_database() {
    let fixture = Fixture::new(Platform::Linux);
    private_dir(fixture.proxy.data_dir(), fixture.proxy.uid).unwrap();
    let database = fixture.proxy.data_dir.join("greyproxy.db");
    fs::write(&database, "unowned state").unwrap();
    assert_eq!(
        fixture.proxy.install().unwrap_err().code(),
        "proxy-configuration-changed"
    );
    assert_eq!(fs::read_to_string(database).unwrap(), "unowned state");
    assert!(!fixture.proxy.home.join("manager.log").exists());
}

#[test]
fn malformed_and_oversized_health_responses_never_pass() {
    let fixture = Fixture::new(Platform::Linux);
    fixture.configure();
    for text in [
        "not JSON\n200".to_owned(),
        format!("{}\n200", "x".repeat(70_000)),
    ] {
        fs::write(fixture.proxy.home.join("response"), text).unwrap();
        executable(&fixture.proxy.curl, "/bin/cat \"$HOME/response\"");
        assert_eq!(
            fixture.proxy.health().unwrap_err().code(),
            "proxy-unhealthy"
        );
    }
}

#[test]
fn an_unloaded_launchd_service_is_stopped_when_its_login_domain_is_available() {
    let fixture = Fixture::new(Platform::MacOs);
    fixture.configure();
    executable(
        &fixture.proxy.service_program,
        "case \"$*\" in *cockpit.greyproxy*) exit 113;; *) exit 0;; esac",
    );
    assert_eq!(
        fixture.proxy.health().unwrap_err().code(),
        "proxy-service-stopped"
    );
    fixture.proxy.stop().unwrap();
}

#[test]
fn a_failed_cold_configuration_write_can_be_retried_without_adopting_state() {
    let fixture = Fixture::new(Platform::Linux);
    let parent = fixture.proxy.unit_path.parent().unwrap();
    fs::create_dir_all(parent).unwrap();
    fs::set_permissions(parent, fs::Permissions::from_mode(0o500)).unwrap();
    assert!(fixture.proxy.install().is_err());
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(!fixture.proxy.configuration_path().exists());
    fixture.proxy.install().unwrap();
}

#[test]
fn version_output_is_bounded_before_service_actions() {
    let fixture = Fixture::new(Platform::Linux);
    let response = fixture.proxy.home.join("version-response");
    fs::write(
        &response,
        format!(
            "greyproxy 0.4.5 (fixture)\n  commit: {}\n{}",
            crate::PINNED_GREYPROXY_COMMIT,
            "x".repeat(70_000)
        ),
    )
    .unwrap();
    executable(
        &fixture.proxy.program,
        &format!("/bin/cat '{}'", response.display()),
    );
    assert!(
        fixture
            .proxy
            .install()
            .unwrap_err()
            .to_string()
            .contains("output exceeded")
    );
    assert!(!fixture.proxy.home.join("manager.log").exists());
}

#[test]
#[cfg(target_os = "linux")]
fn timeout_cleans_up_the_commands_own_descendant() {
    let mut fixture = Fixture::new(Platform::Linux);
    fixture.configure();
    fixture.proxy.command_deadline = Duration::from_millis(80);
    executable(
        &fixture.proxy.service_program,
        "/bin/sleep 30 &\nprintf '%s' \"$!\" > \"$HOME/descendant.pid\"\nexit 0",
    );
    assert!(fixture.proxy.health().is_err());
    let pid = fs::read_to_string(fixture.proxy.home.join("descendant.pid")).unwrap();
    let running =
        fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| !stat.contains(") Z "));
    if running {
        // The RED test must not leave its fixture process behind.
        let _ = Command::new("/bin/kill").args(["-KILL", &pid]).status();
    }
    assert!(
        !running,
        "the fixture descendant survived its command's deadline"
    );
}

#[test]
fn restart_waits_for_launchds_old_job_to_unload() {
    let fixture = Fixture::new(Platform::MacOs);
    fixture.configure();
    let script = format!(
        r#"
case "$1" in
bootout) printf 0 > "$HOME/stopping";;
bootstrap) [ ! -e "$HOME/stopping" ] || exit 5;;
print)
  case "$2" in
  *cockpit.greyproxy*)
    if [ -e "$HOME/stopping" ]; then
      count=$(/bin/cat "$HOME/stopping"); count=$((count + 1))
      if [ "$count" -ge 3 ]; then /bin/rm "$HOME/stopping"; exit 113; fi
      printf '%s' "$count" > "$HOME/stopping"
    fi;;
  esac
  printf '%s' '{}';;
esac
"#,
        fixture.manager_body()
    );
    executable(&fixture.proxy.service_program, &script);
    fixture.proxy.restart().unwrap();
    assert!(!fixture.proxy.home.join("stopping").exists());
}

#[test]
fn service_overrides_are_refused_before_an_api_token_is_sent() {
    let fixture = Fixture::new(Platform::Linux);
    fixture.configure();
    let script = fs::read_to_string(&fixture.proxy.service_program).unwrap();
    fs::write(
        &fixture.proxy.service_program,
        format!("{script}\nprintf 'DropInPaths=/foreign/override.conf\\n'\n"),
    )
    .unwrap();
    assert_eq!(
        fixture.proxy.health().unwrap_err().code(),
        "proxy-unhealthy"
    );
    assert!(!fixture.proxy.home.join("curl.log").exists());
    assert!(fixture.proxy.stop().is_err());
    assert!(fixture.proxy.restart().is_err());
    let log = fs::read_to_string(fixture.proxy.home.join("manager.log")).unwrap();
    assert!(
        !log.lines().any(|line| line.contains("--user stop")),
        "{log}"
    );
}

#[test]
fn process_identity_and_socket_ownership_are_required_before_any_http_request() {
    for name in ["cmdline", "environ", "net/tcp"] {
        let fixture = Fixture::new(Platform::Linux);
        fixture.configure();
        fs::write(
            fixture.proxy.proc_root.join("123").join(name),
            "foreign process",
        )
        .unwrap();
        assert!(fixture.proxy.health().is_err(), "{name}");
        assert!(!fixture.proxy.home.join("curl.log").exists(), "{name}");
    }
    let fixture = Fixture::new(Platform::MacOs);
    fixture.configure();
    executable(
        &fixture.proxy.lsof,
        &format!("printf 'p456\\nn127.0.0.1:{}\\n'", fixture.proxy.ports[0]),
    );
    assert!(fixture.proxy.health().is_err());
    assert!(!fixture.proxy.home.join("curl.log").exists());
}

#[test]
fn an_ipv6_management_listener_is_refused_before_authenticated_http() {
    let fixture = Fixture::new(Platform::Linux);
    fixture.configure();
    let script = fs::read_to_string(&fixture.proxy.curl).unwrap();
    fs::write(&fixture.proxy.curl, script.replace("exit 7", "exit 0")).unwrap();
    assert_eq!(
        fixture.proxy.health().unwrap_err().code(),
        "proxy-endpoint-occupied"
    );
    let log = fs::read_to_string(fixture.proxy.home.join("curl.log")).unwrap();
    assert!(!log.contains("--header"), "{log}");
}

#[test]
fn healthy_management_alone_does_not_prove_the_traffic_listeners() {
    for table in ["tcp", "udp"] {
        let fixture = Fixture::new(Platform::Linux);
        fixture.configure();
        let path = fixture.proxy.proc_root.join("123/net").join(table);
        let text = fs::read_to_string(&path).unwrap();
        fs::write(
            path,
            text.lines()
                .take(2)
                .filter(|line| table == "tcp" || *line == "header")
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();
        assert!(fixture.proxy.health().is_err(), "{table} listener missing");
        assert!(!fixture.proxy.home.join("curl.log").exists());
    }
    let fixture = Fixture::new(Platform::MacOs);
    fixture.configure();
    executable(
        &fixture.proxy.lsof,
        &format!("printf 'p123\nn127.0.0.1:{}\n'", fixture.proxy.ports[0]),
    );
    assert!(fixture.proxy.health().is_err());
}

#[test]
fn launchd_nested_coalitions_do_not_replace_the_jobs_state() {
    let fixture = Fixture::new(Platform::MacOs);
    fixture.configure();
    let body = format!(
        "gui/123/job = {{\n{}\nresource coalition = {{\nstate = active\n}}\n}}\n",
        fixture.manager_body()
    );
    executable(
        &fixture.proxy.service_program,
        &format!("printf '%s' '{body}'"),
    );
    fixture.proxy.health().unwrap();
}

#[test]
fn a_stopped_foreign_loaded_job_is_never_stopped_or_restarted() {
    for platform in [Platform::Linux, Platform::MacOs] {
        let fixture = Fixture::new(platform);
        fixture.configure();
        let body = fixture
            .manager_body()
            .replace("ActiveState=active", "ActiveState=inactive")
            .replace("state = running", "state = waiting")
            .replace(
                &fixture.proxy.unit_path.display().to_string(),
                "/foreign/unit",
            );
        executable(
            &fixture.proxy.service_program,
            &format!("printf '%s\\n' \"$*\" >> \"$HOME/manager.log\"\nprintf '%s' '{body}'"),
        );
        assert!(fixture.proxy.stop().is_err());
        let log = fs::read_to_string(fixture.proxy.home.join("manager.log")).unwrap();
        assert!(!log.contains("stop") && !log.contains("bootout"), "{log}");
    }
}

#[test]
fn reinstall_kickstarts_a_stopped_but_loaded_launchd_job() {
    let fixture = Fixture::new(Platform::MacOs);
    fixture.configure();
    let active = fixture.manager_body();
    let inactive = active
        .replace("state = running", "state = waiting")
        .replace("pid = 123", "pid = 0");
    executable(
        &fixture.proxy.service_program,
        &format!(
            r#"
case "$1" in
print) if [ -f "$HOME/started" ]; then printf '%s' '{active}'; else printf '%s' '{inactive}'; fi;;
bootstrap) exit 5;;
kickstart) /bin/touch "$HOME/started";;
esac
"#
        ),
    );
    fixture.proxy.install().unwrap();
}

#[test]
fn systemd_enables_the_exact_unit_path() {
    let fixture = Fixture::new(Platform::Linux);
    fixture.proxy.install().unwrap();
    let log = fs::read_to_string(fixture.proxy.home.join("manager.log")).unwrap();
    assert!(
        log.contains(&format!(
            "--user enable {}",
            fixture.proxy.unit_path.display()
        )),
        "{log}"
    );
}

#[test]
fn a_new_pin_can_stop_and_explicitly_upgrade_the_previous_owned_service() {
    let fixture = Fixture::new(Platform::Linux);
    fixture.configure();
    let mut upgraded = fixture.proxy.clone();
    upgraded.program = fixture
        .proxy
        .data_dir
        .parent()
        .unwrap()
        .join("engines")
        .join(format!("{}-{}", "0".repeat(64), "1".repeat(64)))
        .join("greyproxy/greyproxy");
    fs::create_dir_all(upgraded.program.parent().unwrap()).unwrap();
    fs::copy(&fixture.proxy.program, &upgraded.program).unwrap();
    assert_eq!(
        upgraded.install().unwrap_err().code(),
        "proxy-engine-restart-required"
    );
    assert_eq!(
        upgraded.health().unwrap_err().code(),
        "proxy-engine-restart-required"
    );
    upgraded.stop().unwrap();
    let body = fixture.manager_body().replace(
        &fixture.proxy.program.display().to_string(),
        &upgraded.program.display().to_string(),
    );
    let old_script = fs::read_to_string(&fixture.proxy.service_program).unwrap();
    let process = fixture.proxy.proc_root.join("123");
    let script = format!(
        r#"
if [ "$2" = start ]; then
  printf '%s' '{body}' > "$HOME/current-report"
  /bin/rm '{process}/exe'
  /bin/ln -s '{program}' '{process}/exe'
  printf '%s\0' '{program}' serve -C '{config}' --require-api-token --require-client-secret > '{process}/cmdline'
fi
if [ "$2" = show ] && [ -f "$HOME/current-report" ]; then /bin/cat "$HOME/current-report"; exit 0; fi
{old_script}
"#,
        process = process.display(),
        program = upgraded.program.display(),
        config = upgraded.configuration_path().display()
    );
    executable(&fixture.proxy.service_program, &script);
    upgraded.restart().unwrap();
    upgraded.health().unwrap();
    let unit = fs::read_to_string(upgraded.unit_path()).unwrap();
    assert!(unit.contains(&upgraded.program.display().to_string()));
    assert!(!unit.contains(&fixture.proxy.program.display().to_string()));
}
