//! `torlek-sandbox run SPEC` with the real bubblewrap on this host: what the mount view
//! leaves an agent in its checkout. Needs `bwrap` on `PATH`, as CI installs it.
#![cfg(target_os = "linux")]

use std::io::{Read, Write};
use std::net::{IpAddr, TcpListener};
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;

use cockpit_sandbox::{
    AllowEntry, AllowSource, Allowlist, CA_BUNDLE_FILE, CREDENTIALS_FILE, CredentialHost,
    CredentialScheme, DECISIONS_FILE, EGRESS_FILE, EgressDecisions, EgressReport, Endpoint,
    Forward, Interception, LaunchSpec, NeedsSource, Protected, REQUESTS_FILE, SESSION_CA_FILE,
    SPEC_FILE, SPEC_VERSION, SpecCredential, Subject, Target, WhenAbsent,
};

const LAUNCHER: &str = env!("CARGO_BIN_EXE_torlek-sandbox");

struct Fixture {
    root: tempfile::TempDir,
    egress: Allowlist,
    interception: Option<Interception>,
    forwards: Vec<Forward>,
}

impl Fixture {
    /// A checkout with a Git folder, in Cargo's temporary folder for tests.
    fn new() -> Fixture {
        let root = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
        let fixture = Fixture {
            root,
            egress: Allowlist {
                program: "sh".to_owned(),
                entries: Vec::new(),
            },
            interception: None,
            forwards: Vec::new(),
        };
        for folder in ["checkout/.git/hooks", "home", "home-source", "mounts"] {
            std::fs::create_dir_all(fixture.path(folder)).unwrap();
        }
        std::fs::write(fixture.path("checkout/.git/config"), "[core]\n").unwrap();
        fixture
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.path().canonicalize().unwrap().join(relative)
    }

    fn spec(&self, arguments: Vec<String>) -> LaunchSpec {
        let checkout = self.path("checkout");
        let protected = [
            (".git/hooks", WhenAbsent::EmptyFolder),
            (".git/config", WhenAbsent::EmptyFolder),
            (".torlek", WhenAbsent::EmptyFolder),
            (".vscode/tasks.json", WhenAbsent::Unprotected),
        ]
        .into_iter()
        .map(|(path, when_absent)| Protected {
            path: checkout.join(path),
            when_absent,
        })
        .collect();
        LaunchSpec {
            version: SPEC_VERSION,
            subject: Subject::Session {
                session: "s1".to_owned(),
            },
            bubblewrap: bubblewrap(),
            landlock_abi: probed_landlock_abi(),
            program: PathBuf::from("/bin/sh"),
            arguments,
            working_dir: checkout.clone(),
            home: self.path("home"),
            home_source: self.path("home-source"),
            read_only: self
                .interception
                .iter()
                .map(|_| self.path("config"))
                .collect(),
            read_only_declared: Vec::new(),
            sockets: Vec::new(),
            read_write: vec![checkout.clone()],
            folders: Vec::new(),
            protected,
            hidden: vec![checkout.join(".env")],
            needs_source: NeedsSource::Floor,
            forwards: self.forwards.clone(),
            login: None,
            egress: self.egress.clone(),
            interception: self.interception.clone(),
        }
    }

    /// What `sh -c line` printed in the sandbox, ending `exit=<code>`.
    fn run(&self, line: &str) -> String {
        self.run_with(vec!["-c".to_owned(), line.to_owned()])
    }

    fn run_with(&self, arguments: Vec<String>) -> String {
        let spec_path = self.write_spec(arguments);
        let output = Command::new(LAUNCHER)
            .arg("run")
            .arg(&spec_path)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .output()
            .unwrap();
        format!(
            "{}{}exit={}\n",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
            output.status.code().unwrap_or(-1)
        )
    }
}

impl Fixture {
    fn write_spec(&self, arguments: Vec<String>) -> PathBuf {
        let spec_path = self.path("mounts").join(SPEC_FILE);
        std::fs::write(
            &spec_path,
            serde_json::to_vec(&self.spec(arguments)).unwrap(),
        )
        .unwrap();
        spec_path
    }

    /// What `sh -c line` printed in the sandbox with a terminal for its standard streams, as
    /// in a tmux pane: run through util-linux's `script`.
    fn run_on_a_terminal(&self, line: &str) -> String {
        let spec_path = self.write_spec(vec!["-c".to_owned(), line.to_owned()]);
        let output = Command::new("script")
            .args(["--quiet", "--return", "--command"])
            .arg(format!("{LAUNCHER} run {}", spec_path.display()))
            .arg("/dev/null")
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .output()
            .expect("util-linux's `script` runs");
        String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n")
    }
}

fn bubblewrap() -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|folder| folder.join("bwrap"))
        .find(|candidate| candidate.is_file())
        .expect("bubblewrap (`bwrap`) on PATH")
}

fn probed_landlock_abi() -> u8 {
    let output = Command::new(LAUNCHER)
        .arg("probe")
        .arg(bubblewrap())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    u8::try_from(report["landlock_abi"].as_u64().unwrap()).unwrap()
}

fn write(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

#[test]
fn the_checkout_is_writable() {
    let fixture = Fixture::new();

    let printed = fixture.run("echo written > written && cat written");

    assert_eq!(printed, "written\nexit=0\n");
}

/// A worktree's `.git` names its Git folder; rewritten, it would point the owner's Git at
/// hooks of the agent's.
#[test]
fn a_worktrees_git_file_is_read_only() {
    let fixture = Fixture::new();
    std::fs::remove_dir_all(fixture.path("checkout/.git")).unwrap();
    write(&fixture.path("checkout/.git"), "gitdir: /elsewhere\n");

    let printed = fixture.run("echo 'gitdir: /planted' 2>/dev/null > .git || echo refused");

    assert!(printed.starts_with("refused\n"), "{printed}");
    assert_eq!(
        std::fs::read_to_string(fixture.path("checkout/.git")).unwrap(),
        "gitdir: /elsewhere\n"
    );
}

/// A hidden `.env` that links to a file in the checkout hides that file too.
#[test]
fn a_hidden_link_hides_its_target() {
    let fixture = Fixture::new();
    write(
        &fixture.path("checkout/config/secrets.env"),
        "TOKEN=secret\n",
    );
    symlink("config/secrets.env", fixture.path("checkout/.env")).unwrap();

    let printed = fixture.run("cat .env config/secrets.env; echo read");

    assert!(printed.starts_with("read\n"), "{printed}");
    assert!(!printed.contains("TOKEN"), "{printed}");
}

/// `.vscode/tasks.json` is protected only when present; without it the editor's folder
/// stays an ordinary folder the agent may remove.
#[test]
fn an_absent_editor_file_leaves_its_folder_removable() {
    let fixture = Fixture::new();
    write(&fixture.path("checkout/.vscode/settings.json"), "{}\n");

    let printed = fixture.run("rm -r .vscode && echo removed");

    assert_eq!(printed, "removed\nexit=0\n");
}

/// Each argument keeps the kernel's per-argument limit (128 KiB) it has unsandboxed, however
/// long they are together.
#[test]
fn long_arguments_reach_the_program_each_on_its_own() {
    let fixture = Fixture::new();
    let long = "x".repeat(100 * 1024);

    let printed = fixture.run_with(vec![
        "-c".to_owned(),
        "echo \"${#1} ${#2}\"".to_owned(),
        "sh".to_owned(),
        long.clone(),
        long,
    ]);

    assert_eq!(printed, "102400 102400\nexit=0\n");
}

/// Tools write to `/dev/stdout` and `/dev/stderr`, which name the pane's terminal.
#[test]
fn a_program_on_a_terminal_writes_to_dev_stdout_and_dev_stderr() {
    let fixture = Fixture::new();

    let printed = fixture.run_on_a_terminal("echo out > /dev/stdout && echo err > /dev/stderr");

    assert_eq!(printed, "out\nerr\n");
}

/// A server on the host's loopback that answers one request with `hello`; its port.
fn hello_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request);
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello");
        }
    });
    port
}

/// `curl` through the session proxy, tunnelling with `CONNECT`: the body, then the proxy's
/// answer to `CONNECT`.
fn curl_through_the_proxy(url: &str) -> String {
    format!(
        "curl --silent --proxytunnel --proxy http://127.0.0.1:3128 --max-time 5 \
         --write-out ' connect=%{{http_connect}}' {url}"
    )
}

fn granted_loopback(port: u16) -> AllowEntry {
    AllowEntry {
        target: Target::Address(IpAddr::from([127, 0, 0, 1])),
        port,
        source: AllowSource::Grant {
            grant: "g1".to_owned(),
        },
    }
}

/// #831: the listener stage 2 created inside reaches the launcher, which connects on the
/// host's network to an address granted as such.
#[test]
fn a_granted_address_is_reached_through_the_session_proxy() {
    let mut fixture = Fixture::new();
    let port = hello_server();
    fixture.egress.entries.push(granted_loopback(port));

    let printed = fixture.run(&curl_through_the_proxy(&format!(
        "http://127.0.0.1:{port}/"
    )));

    assert_eq!(printed, "hello connect=200exit=0\n");
}

/// The host's loopback is refused unless granted by address, whatever the agent asks.
#[test]
fn the_hosts_loopback_is_refused_without_a_grant() {
    let fixture = Fixture::new();
    let port = hello_server();

    let printed = fixture.run(&curl_through_the_proxy(&format!(
        "http://127.0.0.1:{port}/"
    )));

    assert!(printed.contains("connect=403"), "{printed}");
    assert!(!printed.contains("hello"), "{printed}");
}

/// Inside, the agent's own servers on its loopback stay its own: the host's are not there.
#[test]
fn the_hosts_loopback_is_not_reachable_directly() {
    let fixture = Fixture::new();
    let port = hello_server();

    let printed = fixture.run(&format!(
        "curl --silent --max-time 5 http://127.0.0.1:{port}/; echo \" curl=$?\""
    ));

    assert!(!printed.contains("hello"), "{printed}");
    assert!(!printed.contains("curl=0\n"), "{printed}");
}

/// No resolver answers inside: no DNS, and no UDP route out of the namespace.
#[test]
fn no_name_resolves_inside_the_sandbox() {
    let fixture = Fixture::new();

    let printed = fixture.run("getent hosts example.com || echo unresolved");

    assert_eq!(printed, "unresolved\nexit=0\n");
}

/// The launcher keeps its counters beside the spec, in the folder no agent reaches.
#[test]
fn the_launcher_reports_what_it_allowed_and_refused() {
    let mut fixture = Fixture::new();
    let port = hello_server();
    fixture.egress.entries.push(granted_loopback(port));

    fixture.run(&format!(
        "{}; {}",
        curl_through_the_proxy(&format!("http://127.0.0.1:{port}/")),
        curl_through_the_proxy("http://unknown.example.com/")
    ));

    let report: EgressReport =
        serde_json::from_slice(&std::fs::read(fixture.path("mounts").join(EGRESS_FILE)).unwrap())
            .unwrap();
    let allowed = &report.destinations[&format!("127.0.0.1:{port}")];
    assert_eq!((allowed.allowed, allowed.refused), (1, 0));
    assert_eq!(allowed.source.as_deref(), Some("grant g1"));
    assert_eq!(allowed.bytes_in, 43);
    let refused = &report.destinations["unknown.example.com:80"];
    assert_eq!((refused.allowed, refused.refused), (0, 1));
    assert!(!fixture.path("mounts/proxy-handoff").exists());
}

const SECRET: &str = "github_pat_never_inside";

impl Fixture {
    /// Binds a credential to `api.github.com`, its trust files in a read-only `config`
    /// folder, the host's roots a CA of the test's own.
    fn bind_credential(&mut self) {
        let mut params = rcgen::CertificateParams::default();
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        let roots = params
            .self_signed(&rcgen::KeyPair::generate().unwrap())
            .unwrap()
            .pem();
        std::fs::create_dir_all(self.path("config")).unwrap();
        write(&self.path("host-roots.pem"), &roots);
        self.egress.entries.push(api_github_grant());
        self.interception = Some(Interception {
            credentials: vec![SpecCredential {
                name: "the project's GitHub API token".to_owned(),
                hosts: vec![CredentialHost {
                    host: "api.github.com".to_owned(),
                    scheme: CredentialScheme::Bearer,
                }],
            }],
            system_bundle: self.path("host-roots.pem"),
            ca_file: self.path("config").join(SESSION_CA_FILE),
            bundle_file: self.path("config").join(CA_BUNDLE_FILE),
        });
    }

    /// Moves the spec's reach of the bound host into the owner's decisions (#674).
    fn reach_through_decisions(&mut self) {
        self.egress.entries.clear();
        let decisions = EgressDecisions {
            allowed: vec![api_github_grant()],
            denied: Vec::new(),
            asking: Vec::new(),
            services: Default::default(),
        };
        write(
            &self.path("mounts").join(DECISIONS_FILE),
            &serde_json::to_string(&decisions).unwrap(),
        );
    }

    fn write_credentials(&self, values: &[&str]) {
        write(
            &self.path("mounts").join(CREDENTIALS_FILE),
            &serde_json::to_string(values).unwrap(),
        );
    }
}

/// #832: the launcher reads the values and removes their file before the program starts;
/// inside, only the session CA's certificate and the host's roots are readable. The CA's key
/// stays in the launcher's memory: no private key is in those folders either.
#[test]
fn a_bound_credential_leaves_only_the_session_cas_certificate_inside() {
    let mut fixture = Fixture::new();
    fixture.bind_credential();
    fixture.write_credentials(&[SECRET]);
    let config = fixture.path("config");

    let printed = fixture.run(&format!(
        "head -1 {ca}; grep -c 'BEGIN CERTIFICATE' {bundle}; \
         grep -rl {SECRET} {mounts} {config} /proc/self/environ 2>/dev/null; \
         grep -rl 'PRIVATE KEY' {mounts} {config} 2>/dev/null; true",
        ca = config.join(SESSION_CA_FILE).display(),
        bundle = config.join(CA_BUNDLE_FILE).display(),
        mounts = fixture.path("mounts").display(),
        config = config.display(),
    ));

    assert_eq!(printed, "-----BEGIN CERTIFICATE-----\n2\nexit=0\n");
    assert!(!fixture.path("mounts").join(CREDENTIALS_FILE).exists());
}

fn api_github_grant() -> AllowEntry {
    AllowEntry {
        target: Target::Name("api.github.com".to_owned()),
        port: 443,
        source: AllowSource::Grant {
            grant: "g1".to_owned(),
        },
    }
}

/// #832, #674: a host the owner's decisions let through at launch counts as reached.
#[test]
fn a_credential_bound_to_a_host_the_owners_decisions_reach_starts() {
    let mut fixture = Fixture::new();
    fixture.bind_credential();
    fixture.reach_through_decisions();
    fixture.write_credentials(&[SECRET]);

    let printed = fixture.run("echo started");

    assert_eq!(printed, "started\nexit=0\n");
}

/// #857 decision 3: without the decisions, the same spec binds a host it does not reach.
#[test]
fn a_credential_bound_to_a_host_nothing_reaches_is_refused() {
    let mut fixture = Fixture::new();
    fixture.bind_credential();
    fixture.egress.entries.clear();
    fixture.write_credentials(&[SECRET]);

    let printed = fixture.run("echo started");

    assert!(!printed.contains("started"), "{printed}");
    assert!(
        printed.contains("cannot be bound to api.github.com: the session may not reach it"),
        "{printed}"
    );
}

#[test]
fn a_spec_binding_a_credential_without_its_value_is_refused() {
    let mut fixture = Fixture::new();
    fixture.bind_credential();

    let printed = fixture.run("echo started");

    assert!(!printed.contains("started"), "{printed}");
    assert!(printed.contains("binds 1 credentials"), "{printed}");
}

/// A value left beside a spec that binds none is removed, never kept for a later launch.
#[test]
fn a_credentials_file_beside_a_spec_binding_none_is_removed_and_refused() {
    let fixture = Fixture::new();
    fixture.write_credentials(&[SECRET]);

    let printed = fixture.run("echo started");

    assert!(!printed.contains("started"), "{printed}");
    assert!(!printed.contains(SECRET), "{printed}");
    assert!(!fixture.path("mounts").join(CREDENTIALS_FILE).exists());
}

/// #832, #434: a spec the launcher refuses, one an older launcher wrote or a malformed one,
/// still leaves no value behind.
#[test]
fn a_refused_spec_leaves_no_credentials_file() {
    let fixture = Fixture::new();
    fixture.write_credentials(&[SECRET]);
    let spec_path = fixture.path("mounts").join(SPEC_FILE);
    write(&spec_path, "{\"version\": 0}");

    let output = Command::new(LAUNCHER)
        .arg("run")
        .arg(&spec_path)
        .env_clear()
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(125));
    assert!(!fixture.path("mounts").join(CREDENTIALS_FILE).exists());
}

/// A port forward to the host's `target` port, listening on 7979 inside.
fn port_forward(target: u16) -> Forward {
    Forward {
        service: "figcraft".to_owned(),
        listen: Endpoint::Port { port: 7979 },
        target: Endpoint::Port { port: target },
    }
}

impl Fixture {
    /// The owner allowed `forward`'s record for the session, before the launch.
    fn allow_service(&self, forward: &Forward) {
        let decisions = EgressDecisions {
            services: cockpit_sandbox::ServiceDecisions {
                allowed: vec![cockpit_sandbox::AllowedService {
                    service: forward.service.clone(),
                    record: forward.record(),
                    source: AllowSource::Request { request: 5 },
                    live: None,
                }],
                ..Default::default()
            },
            ..EgressDecisions::default()
        };
        write(
            &self.path("mounts").join(DECISIONS_FILE),
            &serde_json::to_string(&decisions).unwrap(),
        );
    }

    fn report(&self) -> EgressReport {
        serde_json::from_slice(&std::fs::read(self.path("mounts").join(EGRESS_FILE)).unwrap())
            .unwrap()
    }
}

/// #880: a granted service answers at its listen port inside the sandbox, relayed to its
/// target on the host's loopback; the target's own port is not there.
#[test]
fn a_granted_port_forward_answers_at_its_listen_port_inside() {
    let mut fixture = Fixture::new();
    let port = hello_server();
    let forward = port_forward(port);
    fixture.allow_service(&forward);
    fixture.forwards.push(forward);

    let printed = fixture.run(&format!(
        "curl --silent --max-time 5 http://127.0.0.1:7979/; echo; \
         curl --silent --max-time 5 http://127.0.0.1:{port}/; echo \" direct=$?\""
    ));

    assert!(printed.starts_with("hello\n"), "{printed}");
    assert!(!printed.contains("direct=0"), "{printed}");
    let counters = &fixture.report().forwards["figcraft"];
    assert_eq!((counters.allowed, counters.refused), (1, 0));
    assert_eq!(counters.bytes_in, 43);
}

/// #880: without a decision the connection is closed at once, and the record is listed for
/// the daemon to ask the owner.
#[test]
fn an_ungranted_forward_is_closed_at_once_and_asks_the_owner() {
    let mut fixture = Fixture::new();
    let forward = port_forward(hello_server());
    fixture.forwards.push(forward.clone());

    let printed =
        fixture.run("curl --silent --max-time 5 http://127.0.0.1:7979/; echo \" curl=$?\"");

    assert!(!printed.contains("hello"), "{printed}");
    assert!(!printed.contains("curl=0"), "{printed}");
    assert!(
        !printed.contains("curl=28"),
        "timed out instead of closed: {printed}"
    );
    let requests: cockpit_sandbox::EgressRequests =
        serde_json::from_slice(&std::fs::read(fixture.path("mounts").join(REQUESTS_FILE)).unwrap())
            .unwrap();
    assert_eq!(
        requests.services,
        vec![cockpit_sandbox::AskedService::of(&forward)]
    );
    assert_eq!(fixture.report().forwards["figcraft"].refused, 1);
}

/// A Unix socket server in `folder`, answering HTTP `hello`; bound through the folder's
/// descriptor, since a test folder's path can pass the kernel's limit for a socket.
fn socket_server(folder: &Path) -> PathBuf {
    let opened = std::fs::File::open(folder).unwrap();
    let listener = std::os::unix::net::UnixListener::bind(format!(
        "/proc/self/fd/{}/hello.sock",
        std::os::fd::AsRawFd::as_raw_fd(&opened)
    ))
    .unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request);
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello");
        }
    });
    folder.join("hello.sock")
}

/// #880: a socket service answers at its listen path, in a folder bound read-only, so the
/// program can neither remove the socket nor put another beside it.
#[test]
fn a_socket_forward_answers_at_its_listen_path_which_the_session_cannot_replace() {
    let mut fixture = Fixture::new();
    std::fs::create_dir_all(fixture.path("service")).unwrap();
    let target = socket_server(&fixture.path("service"));
    let forward = Forward {
        service: "engine".to_owned(),
        listen: Endpoint::Socket {
            path: PathBuf::from("/run/torlek/engine.sock"),
        },
        target: Endpoint::Socket { path: target },
    };
    fixture.allow_service(&forward);
    fixture.forwards.push(forward);

    let printed = fixture.run(
        "curl --silent --max-time 5 --unix-socket /run/torlek/engine.sock http://engine/; echo; \
         rm -f /run/torlek/engine.sock 2>/dev/null; echo rm=$?; \
         touch /run/torlek/other 2>/dev/null; echo touch=$?; \
         test -S /run/torlek/engine.sock && echo still-a-socket",
    );

    assert_eq!(printed, "hello\nrm=1\ntouch=1\nstill-a-socket\nexit=0\n");
}

/// #880: a socket listen inside a folder the view binds would hide part of it; the launch is
/// refused naming the service.
#[test]
fn a_socket_forward_listening_in_the_checkout_is_refused() {
    let mut fixture = Fixture::new();
    fixture.forwards.push(Forward {
        service: "engine".to_owned(),
        listen: Endpoint::Socket {
            path: fixture.path("checkout/engine.sock"),
        },
        target: Endpoint::Port { port: 2375 },
    });

    let printed = fixture.run("echo started");

    assert!(!printed.contains("started"), "{printed}");
    assert!(printed.contains("service engine listens in"), "{printed}");
    assert!(printed.ends_with("exit=125\n"), "{printed}");
}

/// A project engine's relay (#390), the real `torlek-sandbox engine-relay`, serving this
/// test's user, which stands for both the owner and the engine user. Its "engine" is this
/// test's loopback, where `hello_server` listens.
struct EngineRelayProcess {
    _dir: tempfile::TempDir,
    socket: PathBuf,
    child: std::process::Child,
}

impl Drop for EngineRelayProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn own_uid() -> u32 {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(env!("CARGO_TARGET_TMPDIR"))
        .unwrap()
        .uid()
}

fn engine_relay() -> EngineRelayProcess {
    // A socket path must fit the kernel's 108 bytes, which the target folder may not.
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("relay.sock");
    let child = Command::new(LAUNCHER)
        .arg("engine-relay")
        .arg(&socket)
        .arg(own_uid().to_string())
        .spawn()
        .unwrap();
    let started = std::time::Instant::now();
    while std::os::unix::net::UnixStream::connect(&socket).is_err() {
        assert!(started.elapsed().as_secs() < 10, "no engine relay");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    EngineRelayProcess {
        _dir: dir,
        socket,
        child,
    }
}

/// The owner's decisions with `forward` allowed live, and the engine the daemon verified.
fn live_decisions(
    forward: Option<&Forward>,
    engine: Option<cockpit_sandbox::EngineRelay>,
) -> EgressDecisions {
    EgressDecisions {
        services: cockpit_sandbox::ServiceDecisions {
            allowed: forward
                .map(|forward| cockpit_sandbox::AllowedService {
                    service: forward.service.clone(),
                    record: forward.record(),
                    source: AllowSource::Request { request: 9 },
                    live: Some(forward.clone()),
                })
                .into_iter()
                .collect(),
            engine,
            ..Default::default()
        },
        ..EgressDecisions::default()
    }
}

fn relay_of(process: &EngineRelayProcess, uid: u32) -> cockpit_sandbox::EngineRelay {
    cockpit_sandbox::EngineRelay {
        user: "torlek-e3".to_owned(),
        uid,
        socket: process.socket.clone(),
    }
}

impl Fixture {
    fn write_decisions(&self, decisions: &EgressDecisions) {
        write(
            &self.path("mounts").join(DECISIONS_FILE),
            &serde_json::to_string(decisions).unwrap(),
        );
    }

    fn wait_for(&self, marker: &str) {
        let started = std::time::Instant::now();
        while !self.path("checkout").join(marker).exists() {
            assert!(
                started.elapsed().as_secs() < 20,
                "the program never wrote {marker}"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
}

/// `curl` of the sandbox's `port`, retried while the launcher opens it, then its exit status.
fn curl_when_open(port: u16) -> String {
    format!(
        "for try in $(seq 50); do curl --silent --max-time 2 http://127.0.0.1:{port}/ && break; \
         sleep 0.1; done; echo \" curl=$?\""
    )
}

/// #390: a container port the owner allowed opens inside the sandbox at the same port, and
/// reaches the engine's port through its relay.
#[test]
fn an_allowed_container_port_answers_inside_through_the_engines_relay() {
    let fixture = Fixture::new();
    let port = hello_server();
    let relay = engine_relay();
    let forward = Forward::container_port("torlek-e3", port);
    fixture.write_decisions(&live_decisions(
        Some(&forward),
        Some(relay_of(&relay, own_uid())),
    ));

    let printed = fixture.run(&curl_when_open(port));

    assert!(printed.starts_with("hello curl=0\n"), "{printed}");
    let counters = &fixture.report().forwards["containers"];
    assert_eq!((counters.allowed, counters.refused), (1, 0));
}

/// #390: a port opens when the owner allows it while the session runs, and closes when the
/// decision is taken back.
#[test]
fn a_container_port_opens_when_allowed_and_closes_when_revoked() {
    let fixture = Fixture::new();
    let port = hello_server();
    let relay = engine_relay();
    let engine = Some(relay_of(&relay, own_uid()));
    let forward = Forward::container_port("torlek-e3", port);
    fixture.write_decisions(&live_decisions(None, engine.clone()));
    let spec = fixture.write_spec(vec![
        "-c".to_owned(),
        format!(
            "curl --silent --max-time 2 http://127.0.0.1:{port}/; echo \" before=$?\"; \
             touch asked; until [ -e allowed ]; do sleep 0.05; done; {}; \
             touch used; until [ -e revoked ]; do sleep 0.05; done; \
             curl --silent --max-time 2 http://127.0.0.1:{port}/; echo \" after=$?\"",
            curl_when_open(port)
        ),
    ]);
    let launched = Command::new(LAUNCHER)
        .arg("run")
        .arg(&spec)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    fixture.wait_for("asked");
    fixture.write_decisions(&live_decisions(Some(&forward), engine.clone()));
    write(&fixture.path("checkout/allowed"), "");
    fixture.wait_for("used");
    fixture.write_decisions(&live_decisions(None, engine));
    // Two of the launcher's polls of the decisions.
    std::thread::sleep(std::time::Duration::from_millis(600));
    write(&fixture.path("checkout/revoked"), "");
    let output = launched.wait_with_output().unwrap();

    let printed = String::from_utf8_lossy(&output.stdout);
    assert!(
        printed.starts_with(" before=7\nhello curl=0\n"),
        "{printed}"
    );
    assert!(printed.ends_with(" after=7\n"), "{printed}");
}

/// #390: the relay's socket answers only as the engine's user; another user listening there
/// reaches nothing.
#[test]
fn a_relay_run_by_another_user_than_the_engines_is_refused() {
    let fixture = Fixture::new();
    let port = hello_server();
    let relay = engine_relay();
    let forward = Forward::container_port("torlek-e3", port);
    fixture.write_decisions(&live_decisions(
        Some(&forward),
        Some(relay_of(&relay, own_uid() + 1)),
    ));

    let printed = fixture.run(&format!(
        "sleep 1; curl --silent --max-time 2 http://127.0.0.1:{port}/; echo \" curl=$?\""
    ));

    assert!(!printed.contains("hello"), "{printed}");
    let counters = &fixture.report().forwards["containers"];
    assert_eq!(counters.unreachable, 1, "{counters:?}");
    assert!(
        counters
            .last_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("listens on the engine's relay")),
        "{counters:?}"
    );
}

/// #390: a forward of another shape is never opened live, whatever the decisions say; the
/// sandbox's port stays closed.
#[test]
fn only_a_container_port_opens_live() {
    let fixture = Fixture::new();
    let port = hello_server();
    let relay = engine_relay();
    let declared_shape = port_forward(port);
    fixture.write_decisions(&live_decisions(
        Some(&declared_shape),
        Some(relay_of(&relay, own_uid())),
    ));

    let printed = fixture
        .run("sleep 1; curl --silent --max-time 2 http://127.0.0.1:7979/; echo \" curl=$?\"");

    assert!(printed.starts_with(" curl=7\n"), "{printed}");
}

/// #390: the binder's connection to the launcher stays the binder's: the program holds only
/// its standard streams.
#[test]
fn the_program_holds_no_connection_to_the_launcher() {
    let fixture = Fixture::new();

    let printed = fixture.run("ls /proc/$$/fd | tr '\\n' ' '");

    assert_eq!(printed, "0 1 2 exit=0\n");
}
