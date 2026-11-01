//! The profile Cockpit requests: what it carries on each platform, and each rule a platform
//! or request cannot enforce, refused with the rule and the reason rather than weakened.

mod common;

use std::path::{Path, PathBuf};

use cockpit_sandbox::{
    DesktopSession, EndpointKind, FolderMount, Platform, Profile, ProfileRequest, Refusal,
};
use common::{MOUNT_SOURCES, SESSION_DIR, profile_request, wsl2};

fn refusal(request: ProfileRequest, platform: &Platform) -> Refusal {
    Profile::build(request, platform).expect_err("the profile is refused")
}

#[test]
fn the_profile_denies_reading_everything_but_the_checkout_and_cockpits_folders() {
    let profile = Profile::build(profile_request("/home/owner/app"), &Platform::Linux).unwrap();

    let rules = profile.rules();

    for expected in [
        "platform linux",
        "provider claude",
        "read-write /home/owner/app",
        "read-only /run/user/1000/cockpit/agent",
        "read everything else: denied",
        "deny /home/owner/.local/state/cockpit/cockpit.db",
        "deny read ~/.ssh/id_*",
        "deny write .git/hooks",
        "network: through greyproxy only",
        "secret COCKPIT_LAUNCH_TOKEN passed through",
        "desktop session withheld",
    ] {
        assert!(
            rules.iter().any(|rule| rule == expected),
            "{expected} missing from {rules:#?}"
        );
    }
}

#[test]
fn an_unsupported_platform_is_refused_by_name() {
    let refused = refusal(
        profile_request("/home/owner/app"),
        &Platform::Unsupported("freebsd".to_owned()),
    );

    assert_eq!(refused.rule, "sandbox");
    assert!(refused.reason.contains("freebsd"), "{refused}");
}

#[test]
fn a_checkout_inside_a_denied_folder_is_refused_instead_of_handed_over_unreadable() {
    let mut request = profile_request("/home/owner/.local/state/cockpit/app");
    request.denied = vec![PathBuf::from("/home/owner/.local/state/cockpit")];

    let refused = refusal(request, &Platform::Linux);

    assert_eq!(
        refused.rule,
        "read-write /home/owner/.local/state/cockpit/app"
    );
    assert!(
        refused.reason.contains("which the profile denies"),
        "{refused}"
    );
}

#[test]
fn a_relative_path_is_refused() {
    let refused = refusal(profile_request("app"), &Platform::Linux);

    assert_eq!(refused.rule, "read-write app");
}

#[test]
fn a_checkout_on_a_windows_drive_is_refused_under_wsl2() {
    let refused = refusal(profile_request("/mnt/c/Users/owner/app"), &wsl2());

    assert_eq!(refused.rule, "read-write /mnt/c/Users/owner/app");
    assert!(refused.reason.contains("Windows drive"), "{refused}");
}

#[test]
fn wsl2_denies_the_windows_drives_and_interop_and_drops_the_interop_variable() {
    let mut request = profile_request("/home/owner/app");
    request
        .environment
        .push(("WSL_INTEROP".into(), "/run/WSL/1_interop".into()));

    let profile = Profile::build(request, &wsl2()).unwrap();

    for denied in ["/mnt/c", "/mnt/d", "/run/WSL"] {
        assert!(
            profile.denied.contains(&PathBuf::from(denied)),
            "{denied} missing from {:?}",
            profile.denied
        );
    }
    assert!(
        profile
            .environment
            .iter()
            .all(|(name, _)| name != "WSL_INTEROP")
    );
}

#[test]
fn a_withheld_desktop_session_refuses_an_environment_holding_a_desktop_variable() {
    let mut request = profile_request("/home/owner/app");
    request.environment.push(("DISPLAY".into(), ":0".into()));

    let refused = refusal(request, &Platform::Linux);

    assert_eq!(refused.rule, "desktop withheld");
    assert!(refused.reason.contains("DISPLAY"), "{refused}");
}

#[test]
fn a_shared_desktop_session_keeps_its_variables() {
    let mut request = profile_request("/home/owner/app");
    request.desktop = DesktopSession::Shared;
    request.environment.push(("DISPLAY".into(), ":0".into()));

    let profile = Profile::build(request, &Platform::Linux).unwrap();

    assert!(
        profile
            .environment
            .iter()
            .any(|(name, _)| name == "DISPLAY")
    );
    assert!(
        profile
            .rules()
            .contains(&"desktop session shared".to_owned())
    );
}

#[test]
fn a_malformed_allowed_host_is_refused() {
    let mut request = profile_request("/home/owner/app");
    request.allow_hosts = vec!["example.com:http".to_owned()];

    let refused = refusal(request, &Platform::Linux);

    assert_eq!(refused.rule, "allow host example.com:http");
}

#[test]
fn a_provider_name_outside_lowercase_letters_digits_and_dashes_is_refused() {
    let mut request = profile_request("/home/owner/app");
    request.provider = "../claude".to_owned();

    let refused = refusal(request, &Platform::Linux);

    assert_eq!(refused.rule, "provider ../claude");
}

#[test]
fn a_wildcard_host_is_refused_unless_it_is_a_whole_first_label() {
    for host in ["*", "*:443", "ex*ample.com", "example.*"] {
        let mut request = profile_request("/home/owner/app");
        request.allow_hosts = vec![host.to_owned()];

        let refused = refusal(request, &Platform::Linux);

        assert_eq!(refused.rule, format!("allow host {host}"));
    }
    let mut request = profile_request("/home/owner/app");
    request.allow_hosts = vec!["*.example.com:443".to_owned()];
    assert!(Profile::build(request, &Platform::Linux).is_ok());
}

/// The daemon runs a project's `.cockpit/config.toml` checks on the host, direnv runs
/// `.envrc`, IDEs run their run configurations: an agent that could write them would run
/// code with the owner's rights outside the sandbox.
#[test]
fn files_that_run_outside_the_sandbox_are_denied_writing() {
    let profile = Profile::build(profile_request("/home/owner/app"), &Platform::Linux).unwrap();

    let rules = profile.rules();

    for denied in [
        ".cockpit",
        ".envrc",
        ".git/hooks",
        ".vscode/tasks.json",
        ".idea/runConfigurations",
    ] {
        let rule = format!("deny write {denied}");
        assert!(rules.contains(&rule), "{rule} missing from {rules:#?}");
    }
}

/// A provider keeps what reaches the running agent in the session's directory, such as
/// Codex's app-server socket (#437).
#[test]
fn the_sessions_own_directory_is_granted_read_write() {
    let profile = Profile::build(profile_request("/home/owner/app"), &Platform::Linux).unwrap();

    assert!(
        profile.rules().contains(&format!(
            "read-write {SESSION_DIR} (the session's directory)"
        )),
        "{:#?}",
        profile.rules()
    );
}

#[test]
fn a_session_directory_inside_a_denied_folder_is_refused() {
    let mut request = profile_request("/home/owner/app");
    request.denied = vec![PathBuf::from("/run/user/1000/cockpit/sessions")];

    let refused = refusal(request, &Platform::Linux);

    assert_eq!(refused.rule, format!("read-write {SESSION_DIR}"));
}

/// Codex 0.161.0 keeps its sockets in `/tmp/codex-daemon-<uid>/` whatever `TMPDIR` says; the
/// host's folder of that name is the owner's own Codex's, so the sandbox shows a folder of the
/// session's own there (#446).
#[test]
fn a_codex_agent_gets_a_folder_of_its_sessions_own_at_codexs_socket_path() {
    let mut request = profile_request("/home/owner/app");
    request.provider = "codex".to_owned();

    let profile = Profile::build(request, &Platform::Linux).unwrap();

    assert_eq!(
        profile.folder_mounts,
        [FolderMount {
            host: Path::new(MOUNT_SOURCES).join("s1/codex-daemon"),
            sandbox: PathBuf::from("/tmp/codex-daemon-1000"),
        }]
    );
    assert!(
        profile.rules().contains(&format!(
            "folder {MOUNT_SOURCES}/s1/codex-daemon at /tmp/codex-daemon-1000 (codex's own, not the host's)"
        )),
        "{:#?}",
        profile.rules()
    );
}

/// Greywall checks a mount source before it mounts it: one the agent could swap in between,
/// inside or holding a folder it writes, is refused (#446).
#[test]
fn a_mount_source_the_agent_could_swap_is_refused() {
    for mount_dir in [
        format!("{SESSION_DIR}/mounts"),
        "/home/owner/app".to_owned(),
        "/run/user/1000/cockpit".to_owned(),
    ] {
        let mut request = profile_request("/home/owner/app/crate");
        request.provider = "codex".to_owned();
        request.mount_dir = PathBuf::from(&mount_dir);

        let refused = refusal(request, &Platform::Linux);

        assert!(
            refused
                .rule
                .starts_with(&format!("folder {mount_dir}/codex-daemon at ")),
            "{mount_dir}: {refused:?}"
        );
        assert!(refused.reason.contains("could swap"), "{refused:?}");
    }
}

#[test]
fn other_providers_get_no_folder_mount() {
    let profile = Profile::build(profile_request("/home/owner/app"), &Platform::Linux).unwrap();

    assert!(profile.folder_mounts.is_empty());
}

#[test]
fn a_mounted_folders_entry_has_one_host_name_and_nothing_else_has_one() {
    let mount = FolderMount {
        host: PathBuf::from("/run/user/1000/cockpit/sessions/s1/codex-daemon"),
        sandbox: PathBuf::from("/tmp/codex-daemon-1000"),
    };

    assert_eq!(
        mount.host_path(Path::new("/tmp/codex-daemon-1000/5f2c")),
        Some(PathBuf::from(
            "/run/user/1000/cockpit/sessions/s1/codex-daemon/5f2c"
        ))
    );
    for elsewhere in [
        "/tmp/codex-daemon-1000",
        "/tmp/codex-daemon-1000/a/b",
        "/tmp/codex-daemon-1000/..",
        "/tmp/codex-daemon-10000/5f2c",
        "/home/owner/.ssh/agent.sock",
        "codex-daemon-1000/5f2c",
    ] {
        assert_eq!(mount.host_path(Path::new(elsewhere)), None, "{elsewhere}");
    }
}

fn shared_desktop(variables: &[(&str, &str)]) -> ProfileRequest {
    let mut request = profile_request("/home/owner/app");
    request.desktop = DesktopSession::Shared;
    request.environment.extend(
        variables
            .iter()
            .map(|(name, value)| ((*name).into(), (*value).into())),
    );
    request
}

/// A shared desktop session reaches each socket and file its variables name, and the
/// evidence names each one (#435).
#[test]
fn a_shared_desktop_reaches_exactly_the_endpoints_its_variables_name() {
    let request = shared_desktop(&[
        ("XDG_RUNTIME_DIR", "/run/user/1000"),
        ("DISPLAY", ":1.0"),
        ("WAYLAND_DISPLAY", "wayland-1"),
        ("XAUTHORITY", "/run/user/1000/xauth_abc"),
        (
            "DBUS_SESSION_BUS_ADDRESS",
            "unix:path=/run/user/1000/bus,guid=42",
        ),
    ]);

    let profile = Profile::build(request, &Platform::Linux).unwrap();

    let endpoints: Vec<(&str, PathBuf, EndpointKind)> = profile
        .desktop_endpoints
        .iter()
        .map(|endpoint| (endpoint.variable, endpoint.path.clone(), endpoint.kind))
        .collect();
    assert_eq!(
        endpoints,
        [
            ("DISPLAY", "/tmp/.X11-unix/X1".into(), EndpointKind::Socket),
            (
                "WAYLAND_DISPLAY",
                "/run/user/1000/wayland-1".into(),
                EndpointKind::Socket
            ),
            (
                "XAUTHORITY",
                "/run/user/1000/xauth_abc".into(),
                EndpointKind::File
            ),
            (
                "DBUS_SESSION_BUS_ADDRESS",
                "/run/user/1000/bus".into(),
                EndpointKind::Bus
            ),
        ]
    );
    let rules = profile.rules();
    for expected in [
        "desktop socket /tmp/.X11-unix/X1 for DISPLAY",
        "desktop socket /run/user/1000/wayland-1 for WAYLAND_DISPLAY",
        "desktop file /run/user/1000/xauth_abc for XAUTHORITY",
        "desktop bus /run/user/1000/bus for DBUS_SESSION_BUS_ADDRESS",
    ] {
        assert!(
            rules.iter().any(|rule| rule == expected),
            "{expected} missing from {rules:#?}"
        );
    }
}

#[test]
fn a_withheld_desktop_session_reaches_no_endpoint() {
    let profile = Profile::build(profile_request("/home/owner/app"), &Platform::Linux).unwrap();

    assert!(profile.desktop_endpoints.is_empty());
}

#[test]
fn an_absolute_wayland_socket_and_an_xquartz_display_are_their_own_paths() {
    let request = shared_desktop(&[
        (
            "DISPLAY",
            "/private/tmp/com.apple.launchd.abc/org.xquartz:0",
        ),
        ("WAYLAND_DISPLAY", "/run/user/1000/wayland-0"),
    ]);

    let profile = Profile::build(request, &Platform::MacOs).unwrap();

    let paths: Vec<PathBuf> = profile
        .desktop_endpoints
        .iter()
        .map(|endpoint| endpoint.path.clone())
        .collect();
    assert_eq!(
        paths,
        [
            PathBuf::from("/private/tmp/com.apple.launchd.abc/org.xquartz:0"),
            PathBuf::from("/run/user/1000/wayland-0"),
        ]
    );
}

/// Each variable whose endpoint the sandbox cannot reach as named refuses the launch, naming
/// the variable and its value, instead of passing a variable that leads nowhere.
#[test]
fn a_desktop_endpoint_the_sandbox_cannot_reach_is_refused() {
    for (variables, rule) in [
        (
            vec![("DISPLAY", "remote.example.com:0")],
            "desktop DISPLAY=remote.example.com:0",
        ),
        (vec![("DISPLAY", "0")], "desktop DISPLAY=0"),
        (
            vec![("WAYLAND_DISPLAY", "wayland-1")],
            "desktop WAYLAND_DISPLAY=wayland-1",
        ),
        (
            vec![
                ("XDG_RUNTIME_DIR", "/run/user/1000"),
                ("WAYLAND_DISPLAY", "../../etc/x"),
            ],
            "desktop WAYLAND_DISPLAY=/run/user/1000/../../etc/x",
        ),
        (
            vec![("XAUTHORITY", ".Xauthority")],
            "desktop XAUTHORITY=.Xauthority",
        ),
        (
            vec![("DBUS_SESSION_BUS_ADDRESS", "unix:abstract=/tmp/dbus-1")],
            "desktop DBUS_SESSION_BUS_ADDRESS=unix:abstract=/tmp/dbus-1",
        ),
        (
            vec![("DBUS_SESSION_BUS_ADDRESS", "tcp:host=localhost,port=1")],
            "desktop DBUS_SESSION_BUS_ADDRESS=tcp:host=localhost,port=1",
        ),
    ] {
        let refused = refusal(shared_desktop(&variables), &Platform::Linux);

        assert_eq!(refused.rule, rule, "{refused}");
    }
}

#[test]
fn a_desktop_endpoint_inside_a_denied_folder_is_refused() {
    let mut request = shared_desktop(&[("XAUTHORITY", "/home/owner/.local/state/cockpit/x")]);
    request.denied = vec![PathBuf::from("/home/owner/.local/state/cockpit")];

    let refused = refusal(request, &Platform::Linux);

    assert_eq!(refused.rule, "desktop /home/owner/.local/state/cockpit/x");
}
