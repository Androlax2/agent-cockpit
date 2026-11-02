//! Service and process-metadata fixtures for the actual daemon. These commands never
//! contact a host service, inspect a real PID or make HTTP requests.
use std::fs;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::Path;

const PID: u32 = 999_999_999;

pub(super) fn commands(home: &Path) {
    fs::create_dir_all(home.join("bin")).unwrap();
    let manager = r#"#!/bin/sh
fixture="$HOME/proxy-fixture/$COCKPIT_PROFILE"
case "$*" in
  '--user show cockpit-greyproxy-'*|print*io.github.androlax2.cockpit.greyproxy.*)
    if [ -f "$fixture/stopped" ]; then
      case "$*" in print*) exit 113;; esac
      while IFS= read -r line; do
        case "$line" in ActiveState=*) line=ActiveState=inactive;; MainPID=*) line=MainPID=0;; esac
        printf '%s\n' "$line"
      done < "$fixture/manager"
    else /bin/cat "$fixture/manager"; fi;;
  'print gui/'*) exit 0;;
  *) exit 91;;
esac
"#;
    for (name, script) in [
        ("systemctl", manager),
        ("launchctl", manager),
        (
            "curl",
            r#"#!/bin/sh
case "$*" in *'http://[::1]:'*) exit 7;; esac
fixture="$HOME/proxy-fixture/$COCKPIT_PROFILE"
case "$*" in
  */api/health)
    status=ok
    if [ -f "$fixture/unhealthy" ]; then status=unhealthy; fi
    printf '{"service":"greyproxy","status":"%s","ports":{"api":43080,"http-proxy":43051,"socks5-proxy":43052,"dns-proxy-udp":43053,"dns-proxy-tcp":43053}}\n200' "$status";;
  */api/settings)
    case "$*" in *'--header @-'*) /bin/cat > /dev/null; printf '{}\n200';; *) printf '{}\n401';; esac;;
  *) exit 92;;
esac
"#,
        ),
        (
            "readlink",
            r#"#!/bin/sh
case "$*" in
  /proc/999999999/exe) exec /bin/cat "$HOME/proxy-fixture/$COCKPIT_PROFILE/exe";;
  *) exec /usr/bin/readlink "$@";;
esac
"#,
        ),
        (
            "cat",
            r#"#!/bin/sh
case "$*" in
  /proc/999999999/cmdline) exec /bin/cat "$HOME/proxy-fixture/$COCKPIT_PROFILE/cmdline";;
  /proc/999999999/environ) exec /bin/cat "$HOME/proxy-fixture/$COCKPIT_PROFILE/environ";;
  /proc/999999999/net/tcp) exec /bin/cat "$HOME/proxy-fixture/$COCKPIT_PROFILE/tcp";;
  /proc/999999999/net/udp) exec /bin/cat "$HOME/proxy-fixture/$COCKPIT_PROFILE/udp";;
  *) exec /bin/cat "$@";;
esac
"#,
        ),
        (
            "ls",
            r#"#!/bin/sh
case "$*" in
  '-l /proc/999999999/fd') printf '7 -> socket:[100]\n8 -> socket:[101]\n9 -> socket:[102]\n10 -> socket:[103]\n11 -> socket:[104]\n';;
  *) exec /bin/ls "$@";;
esac
"#,
        ),
        (
            "lsof",
            "#!/bin/sh\nprintf 'p999999999\nn127.0.0.1:43080\nn127.0.0.1:43051\nn127.0.0.1:43052\nn127.0.0.1:43053\n'\n",
        ),
    ] {
        let path = home.join("bin").join(name);
        fs::write(&path, script).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
}

pub(super) fn configure(home: &Path, profile: &str) -> cockpit_sandbox::ManagedProxy {
    let environment = cockpit_config::Environment {
        home: Some(home.into()),
        xdg_state_home: Some(home.join("state").into()),
        xdg_config_home: Some(home.join("config").into()),
        profile: cockpit_config::Sourced::new(
            cockpit_config::Profile::parse(profile).unwrap(),
            cockpit_config::Source::ProductDefault,
        ),
        ..cockpit_config::Environment::default()
    };
    let platform = cockpit_sandbox::Platform::detect();
    let proxy = cockpit_sandbox::ManagedProxy::new(&environment, &platform).unwrap();
    for (path, text) in proxy.configuration_files().unwrap() {
        if !path.exists() {
            private_file(&path, &text);
        }
    }
    private_file(&proxy.data_dir().join("api-token"), &"a".repeat(64));
    // Fixture CA material only; the fake service does not parse TLS or open sockets.
    for (name, kind) in [
        ("ca-cert.pem", "CERTIFICATE"),
        ("ca-key.pem", "EC PRIVATE KEY"),
    ] {
        private_file(
            &proxy.data_dir().join(name),
            &format!("-----BEGIN {kind}-----\nfixture\n-----END {kind}-----\n"),
        );
    }
    let state = cockpit_config::Layout::resolve(&environment, home)
        .state_dir
        .value;
    let program = cockpit_sandbox::install::programs(&state)
        .unwrap()
        .greyproxy;
    let config = proxy.data_dir().join("config.json");
    let fixture = home.join("proxy-fixture").join(profile);
    let manager = if platform == cockpit_sandbox::Platform::MacOs {
        format!(
            "state = running\npid = {PID}\npath = {}\nprogram = {}\narguments = {{\n{}\nserve\n-C\n{}\n--require-api-token\n--require-client-secret\n}}\nenvironment = {{\nGREYPROXY_DATA_HOME => {}\n}}\n",
            proxy.unit_path().display(),
            program.display(),
            program.display(),
            config.display(),
            proxy.data_dir().display()
        )
    } else {
        format!(
            "ActiveState=active\nSubState=running\nMainPID={PID}\nNeedDaemonReload=no\nDropInPaths=\nFragmentPath={}\n",
            proxy.unit_path().display()
        )
    };
    for (name, text) in [
        ("manager", manager),
        (
            "exe",
            format!("{}\n", program.canonicalize().unwrap().display()),
        ),
        (
            "cmdline",
            format!(
                "{}\0serve\0-C\0{}\0--require-api-token\0--require-client-secret\0",
                program.display(),
                config.display()
            ),
        ),
        (
            "environ",
            format!("GREYPROXY_DATA_HOME={}\0", proxy.data_dir().display()),
        ),
        (
            "tcp",
            format!(
                "header\n{}",
                [43080, 43051, 43052, 43053]
                    .iter()
                    .enumerate()
                    .map(|(i, port)| format!(
                        "{i}: 0100007F:{port:04X} 00000000:0000 0A 0 0 0 1000 0 {}\n",
                        100 + i
                    ))
                    .collect::<String>()
            ),
        ),
        (
            "udp",
            format!(
                "header\n0: 0100007F:{:04X} 00000000:0000 07 0 0 0 1000 0 104\n",
                43053
            ),
        ),
    ] {
        private_file(&fixture.join(name), &text);
    }
    proxy
}

fn private_file(path: &Path, text: &str) {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path.parent().unwrap())
        .unwrap();
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}
