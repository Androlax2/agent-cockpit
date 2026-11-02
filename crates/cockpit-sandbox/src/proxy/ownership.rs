//! Bind health to the owned process before sending its management credential.
use std::collections::BTreeSet;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use super::{ManagedProxy, ProxyError, command, service::ServiceManager};

impl ManagedProxy {
    pub(super) fn verify_endpoint_owner(&self, pid: u32) -> Result<(), ProxyError> {
        match self.manager {
            ServiceManager::Systemd => self.linux_owner(pid),
            ServiceManager::Launchd => {
                // lsof is supplied by macOS; every configured listener must belong to the
                // loaded PID before Greywall sends any credentials or traffic to it.
                self.macos_listeners(pid, "TCP", &self.ports)?;
                self.macos_listeners(pid, "UDP", &self.ports[3..])
            }
        }
    }

    fn linux_owner(&self, pid: u32) -> Result<(), ProxyError> {
        let process = self.proc_root.join(pid.to_string());
        let deadline = Instant::now() + self.command_deadline;
        self.linux_process_owner(pid, deadline)?;
        let mut sockets = BTreeSet::new();
        let descriptors = self.metadata("ls", &process.join("fd"), deadline, None)?;
        for line in String::from_utf8_lossy(&descriptors).lines() {
            if let Some(inode) = line
                .rsplit_once(" -> ")
                .map(|(_, target)| target)
                .and_then(|target| target.strip_prefix("socket:["))
                .and_then(|target| target.strip_suffix(']'))
            {
                sockets.insert(inode.to_owned());
            }
        }
        for (table, state, ports) in [
            ("tcp", "0A", &self.ports[..]),
            ("udp", "07", &self.ports[3..]),
        ] {
            let endpoints = ports
                .iter()
                .map(|port| format!("0100007F:{port:04X}"))
                .collect::<Vec<_>>();
            let keep = |line: &[u8]| {
                std::str::from_utf8(line).is_ok_and(|line| {
                    let mut fields = line.split_whitespace();
                    fields.nth(1).is_some_and(|local| {
                        endpoints
                            .iter()
                            .any(|endpoint| local.eq_ignore_ascii_case(endpoint))
                    }) && fields.nth(1) == Some(state)
                })
            };
            let bytes = self.metadata(
                "cat",
                &process.join("net").join(table),
                deadline,
                Some(&keep),
            )?;
            let text = String::from_utf8_lossy(&bytes);
            for endpoint in endpoints {
                if !text.lines().any(|line| {
                    let fields: Vec<_> = line.split_whitespace().collect();
                    fields.len() > 9
                        && fields[1].eq_ignore_ascii_case(&endpoint)
                        && fields[3] == state
                        && sockets.contains(fields[9])
                }) {
                    return Err(ownership_error());
                }
            }
        }
        Ok(())
    }

    fn macos_listeners(&self, pid: u32, protocol: &str, ports: &[u16]) -> Result<(), ProxyError> {
        let mut invocation = Command::new(&self.lsof);
        invocation.args([
            "-nP",
            "-a",
            "-p",
            &pid.to_string(),
            &format!("-i{protocol}@127.0.0.1"),
            "-Fpn",
        ]);
        if protocol == "TCP" {
            invocation.arg("-sTCP:LISTEN");
        }
        let output = command::run(&mut invocation, None, self.command_deadline)
            .map_err(ProxyError::Unhealthy)?;
        let text = String::from_utf8_lossy(&output.stdout);
        if !output.status.success()
            || !text.lines().any(|line| line == format!("p{pid}"))
            || ports.iter().any(|port| {
                !text
                    .lines()
                    .any(|line| line == format!("n127.0.0.1:{port}"))
            })
        {
            return Err(ownership_error());
        }
        Ok(())
    }

    /// A broken owned listener must remain stoppable; process identity is enough for
    /// service actions, whereas sending its token also requires socket ownership.
    pub(super) fn verify_service_process(&self, pid: u32) -> Result<(), ProxyError> {
        match self.manager {
            ServiceManager::Systemd => {
                self.linux_process_owner(pid, Instant::now() + self.command_deadline)
            }
            // service_pid already checks launchd's loaded program, arguments and environment.
            ServiceManager::Launchd => Ok(()),
        }
    }

    fn linux_process_owner(&self, pid: u32, deadline: Instant) -> Result<(), ProxyError> {
        let process = self.proc_root.join(pid.to_string());
        // Standard OS commands are bounded and replaceable on a fixture PATH. No test
        // needs to inspect a real user's /proc entries or a live service.
        let executable = self.metadata("readlink", &process.join("exe"), deadline, None)?;
        let executable = std::str::from_utf8(&executable).map_err(|_| ownership_error())?;
        if Some(Path::new(executable.strip_suffix('\n').unwrap_or(executable)).to_owned())
            != self.program.canonicalize().ok()
        {
            return Err(ownership_error());
        }
        let cmdline = self.metadata("cat", &process.join("cmdline"), deadline, None)?;
        let configuration = self.configuration_path();
        let expected: [&[u8]; 6] = [
            self.program.as_os_str().as_bytes(),
            b"serve",
            b"-C",
            configuration.as_os_str().as_bytes(),
            b"--require-api-token",
            b"--require-client-secret",
        ];
        if cmdline
            .split(|byte| *byte == 0)
            .filter(|word| !word.is_empty())
            .ne(expected)
        {
            return Err(ownership_error());
        }
        let environment = self.metadata("cat", &process.join("environ"), deadline, None)?;
        let mut data_home = b"GREYPROXY_DATA_HOME=".to_vec();
        data_home.extend_from_slice(self.data_dir.as_os_str().as_bytes());
        if !environment
            .split(|byte| *byte == 0)
            .any(|value| value == data_home)
        {
            return Err(ownership_error());
        }
        Ok(())
    }

    fn metadata(
        &self,
        tool: &str,
        path: &Path,
        deadline: Instant,
        keep_line: Option<command::LineFilter<'_>>,
    ) -> Result<Vec<u8>, ProxyError> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(ownership_error());
        }
        let mut invocation = Command::new(self.metadata_commands.join(tool));
        invocation
            .env("HOME", &self.home)
            .env("COCKPIT_PROFILE", self.profile.name())
            .env("LC_ALL", "C");
        if tool == "ls" {
            invocation.arg("-l");
        }
        let output = command::run_filtered(invocation.arg(path), None, remaining, keep_line)
            .map_err(|_| ownership_error())?;
        if !output.status.success() {
            return Err(ownership_error());
        }
        Ok(output.stdout)
    }

    pub(super) fn refuse_ipv6_management_listener(&self) -> Result<(), ProxyError> {
        // Greywall currently registers at localhost. Its IPv6 alternative must not point
        // at a different service while our health probe checks the owned IPv4 listener.
        let output = command::run(
            Command::new(&self.curl).env("HOME", &self.home).args([
                "-q",
                "--silent",
                "--noproxy",
                "*",
                "--proxy",
                "",
                "--proto",
                "=http",
                "--connect-timeout",
                "1",
                "--max-time",
                "1",
                "--max-filesize",
                "1024",
                &format!("http://[::1]:{}/api/health", self.ports[0]),
            ]),
            None,
            self.command_deadline.min(Duration::from_secs(2)),
        )
        .map_err(ProxyError::Unhealthy)?;
        if output.status.code() == Some(7) {
            return Ok(());
        }
        Err(ProxyError::EndpointOccupied(format!(
            "[::1]:{}/tcp",
            self.ports[0]
        )))
    }
}

fn ownership_error() -> ProxyError {
    ProxyError::Unhealthy("cannot establish that the pinned process with the owned configuration holds every configured TCP/UDP endpoint".into())
}
