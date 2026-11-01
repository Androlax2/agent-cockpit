//! The Greywall backend: the pinned builds it accepts, the host capabilities it requires,
//! the command and settings file it plans for a profile, and the per-launch greyproxy client
//! identity that keeps back-to-back launches from replacing each other's client secret.

mod common;

use std::collections::HashSet;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use cockpit_sandbox::{
    ClientIdentity, DesktopSession, EngineBuild, Greywall, GreywallPrograms,
    PINNED_GREYPROXY_COMMIT, PINNED_GREYWALL_COMMIT, Plan, Platform, Profile, ProfileRequest,
    Refusal, parse_greyproxy_version, parse_greywall_version,
};
use common::{
    LINUX_CAPABILITIES, SESSION_DIR, capabilities, engine_with, linux_engine, pinned_greyproxy,
    pinned_greywall, profile_request, programs,
};

const GREYWALL_VERSION_OUTPUT: &str = "greywall - lightweight, container-free sandbox
  Version: 0.3.7+dotfiles
  Built:   2026-10-30T09:12:44Z
  Commit:  7c06d51afd26cbc90a32c82375a393570c22ffc7

Platform: linux
  \u{2713} bubblewrap (bwrap 0.11.0)
  \u{2713} landlock (ABI v6)
  \u{2713} seccomp
  \u{2717} D-Bus isolated (xdg-dbus-proxy not found)
  \u{2713} network isolation
";

const GREYPROXY_VERSION_OUTPUT: &str = "greyproxy 0.4.5 (linux/amd64)
commit: cdd5cde76dce1262c0fd8d6b1ab2bd9953df5e71
";

#[test]
fn greywalls_version_output_gives_its_build_and_each_capability() {
    let (build, capabilities) = parse_greywall_version(GREYWALL_VERSION_OUTPUT).unwrap();

    assert_eq!(build, pinned_greywall());
    let dbus = capabilities
        .iter()
        .find(|capability| capability.name == "D-Bus isolated")
        .unwrap();
    assert!(!dbus.available);
    assert_eq!(dbus.detail.as_deref(), Some("xdg-dbus-proxy not found"));
    assert_eq!(capabilities.len(), 5);
}

#[test]
fn greyproxys_version_output_gives_its_build() {
    assert_eq!(
        parse_greyproxy_version(GREYPROXY_VERSION_OUTPUT).unwrap(),
        pinned_greyproxy()
    );
}

#[test]
fn version_output_without_a_commit_is_refused() {
    assert!(parse_greywall_version("Version: 0.3.7\n").is_err());
    assert!(parse_greyproxy_version("greyproxy 0.4.5\n").is_err());
}

#[test]
fn a_greywall_build_other_than_the_pinned_one_is_refused_naming_both_commits() {
    let unpinned = EngineBuild {
        version: "0.3.8".to_owned(),
        commit: "0000000000000000000000000000000000000000".to_owned(),
    };

    let reason = Greywall::from_builds(
        programs(),
        unpinned,
        pinned_greyproxy(),
        Vec::new(),
        Platform::Linux,
        "/tmp/settings".into(),
    )
    .unwrap_err();

    assert!(
        reason.contains("0000000000000000000000000000000000000000"),
        "{reason}"
    );
    assert!(reason.contains(PINNED_GREYWALL_COMMIT), "{reason}");
}

#[test]
fn a_greyproxy_build_other_than_the_pinned_one_is_refused() {
    let unpinned = EngineBuild {
        version: "0.4.6".to_owned(),
        commit: "1111111111111111111111111111111111111111".to_owned(),
    };

    let reason = Greywall::from_builds(
        programs(),
        pinned_greywall(),
        unpinned,
        Vec::new(),
        Platform::Linux,
        "/tmp/settings".into(),
    )
    .unwrap_err();

    assert!(reason.contains(PINNED_GREYPROXY_COMMIT), "{reason}");
}

/// The pins are the commits the release builds (`packages/*/build.sh`), so a bumped package
/// without a bumped pin (or the reverse) fails here instead of refusing every launch.
#[test]
fn the_pins_are_the_commits_the_packages_build() {
    let packages = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages");
    for (script, variable, pinned) in [
        (
            "greywall/build.sh",
            "greywall_commit=",
            PINNED_GREYWALL_COMMIT,
        ),
        (
            "greyproxy/build.sh",
            "greyproxy_commit=",
            PINNED_GREYPROXY_COMMIT,
        ),
    ] {
        let build = std::fs::read_to_string(packages.join(script)).unwrap();
        let built = build
            .lines()
            .find_map(|line| line.strip_prefix(variable))
            .and_then(|rest| rest.split_whitespace().next())
            .unwrap_or_else(|| panic!("{script} sets no {variable}"));

        assert_eq!(built, pinned, "{script}");
    }
}

fn write_program(dir: &Path, name: &str, output: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\ncat <<'EOF'\n{output}EOF\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[test]
fn detect_reads_both_programs_versions() {
    let bin = tempfile::tempdir().unwrap();
    let programs = GreywallPrograms {
        greywall: write_program(bin.path(), "greywall", GREYWALL_VERSION_OUTPUT),
        greyproxy: write_program(bin.path(), "greyproxy", GREYPROXY_VERSION_OUTPUT),
    };

    let engine = Greywall::detect(programs, Platform::Linux, bin.path().join("settings")).unwrap();

    assert_eq!(
        engine.engine(),
        "greywall 0.3.7+dotfiles (7c06d51), greyproxy 0.4.5 (cdd5cde)"
    );
    assert!(
        engine
            .observations()
            .contains(&"D-Bus isolated: no (xdg-dbus-proxy not found)".to_owned())
    );
}

#[test]
fn programs_are_looked_up_on_path_and_a_missing_one_is_named() {
    let bin = tempfile::tempdir().unwrap();
    write_program(bin.path(), "greywall", GREYWALL_VERSION_OUTPUT);

    let reason = GreywallPrograms::on_path(Some(&bin.path().as_os_str().to_owned())).unwrap_err();

    assert_eq!(reason, "no `greyproxy` on PATH");
}

#[test]
fn a_missing_host_capability_refuses_the_rule_it_enforces() {
    let settings = tempfile::tempdir().unwrap();
    let engine = engine_with(
        Platform::Linux,
        capabilities(
            &["bubblewrap", "landlock", "seccomp"],
            &["network isolation"],
        ),
        settings.path(),
    );
    let profile = Profile::build(profile_request("/home/owner/app"), &Platform::Linux).unwrap();

    let refused = engine
        .plan(&profile, &ClientIdentity::for_launch("s1").unwrap())
        .unwrap_err();

    assert_eq!(refused.rule, "network: through greyproxy only");
    assert!(
        refused.reason.contains("network isolation unavailable"),
        "{refused}"
    );
}

#[test]
fn a_provider_without_a_reviewed_engine_profile_is_refused() {
    let settings = tempfile::tempdir().unwrap();
    let mut request = profile_request("/home/owner/app");
    request.provider = "aider".to_owned();
    let profile = Profile::build(request, &Platform::Linux).unwrap();

    let refused = linux_engine(settings.path())
        .plan(&profile, &ClientIdentity::for_launch("s1").unwrap())
        .unwrap_err();

    assert_eq!(refused.rule, "provider aider");
}

#[test]
fn the_plan_runs_the_agent_under_greywall_as_a_proxy_client_of_its_own() {
    let settings = tempfile::tempdir().unwrap();
    let profile = Profile::build(profile_request("/home/owner/app"), &Platform::Linux).unwrap();
    let client = ClientIdentity::for_launch("s1").unwrap();

    let plan = linux_engine(settings.path())
        .plan(&profile, &client)
        .unwrap();

    let arguments: Vec<String> = plan
        .arguments
        .iter()
        .map(|word| word.to_string_lossy().into_owned())
        .collect();
    let settings_path = settings.path().join(format!("{}.json", client.as_str()));
    assert_eq!(plan.program, Path::new("/opt/sandbox/greywall"));
    assert_eq!(
        arguments,
        [
            "--settings",
            settings_path.to_str().unwrap(),
            "--auto-profile",
            "--profile",
            "claude",
            "--proxy-user",
            client.as_str(),
            "--allow-path",
            "/home/owner/app",
            "--allow-path",
            SESSION_DIR,
            "--allow-read-path",
            "/run/user/1000/cockpit/agent",
            "--ignore-secret",
            "COCKPIT_LAUNCH_TOKEN",
            "--",
        ]
    );
    assert!(
        plan.environment
            .contains(&("GREYWALL_NO_PROMPT".into(), "1".into()))
    );
}

#[test]
fn the_settings_deny_reading_by_default_and_list_every_denial() {
    let settings = tempfile::tempdir().unwrap();
    let profile = Profile::build(profile_request("/home/owner/app"), &Platform::Linux).unwrap();

    let plan = linux_engine(settings.path())
        .plan(&profile, &ClientIdentity::for_launch("s1").unwrap())
        .unwrap();

    let written: serde_json::Value = serde_json::from_str(&plan.settings).unwrap();
    assert_eq!(written["filesystem"]["defaultDenyRead"], true);
    let denied_read = written["filesystem"]["denyRead"].as_array().unwrap();
    assert!(denied_read.contains(&"/home/owner/.local/state/cockpit/cockpit.db".into()));
    assert!(denied_read.contains(&"~/.ssh/id_*".into()));
    let denied_write = written["filesystem"]["denyWrite"].as_array().unwrap();
    assert!(denied_write.contains(&".git/hooks".into()));
}

#[test]
fn every_rule_names_what_enforces_it() {
    let settings = tempfile::tempdir().unwrap();
    let profile = Profile::build(profile_request("/home/owner/app"), &Platform::Linux).unwrap();

    let plan = linux_engine(settings.path())
        .plan(&profile, &ClientIdentity::for_launch("s1").unwrap())
        .unwrap();

    let rules: Vec<&str> = plan
        .enforced_by
        .iter()
        .map(|(rule, _)| rule.as_str())
        .collect();
    assert_eq!(rules, profile.rules());
    assert!(plan.enforced_by.contains(&(
        "read everything else: denied".to_owned(),
        "greywall bubblewrap+Landlock (defaultDenyRead)".to_owned()
    )));
}

#[test]
fn macos_needs_no_linux_capabilities() {
    let settings = tempfile::tempdir().unwrap();
    let engine = engine_with(Platform::MacOs, Vec::new(), settings.path());
    let profile = Profile::build(profile_request("/Users/owner/app"), &Platform::MacOs).unwrap();

    let plan = engine
        .plan(&profile, &ClientIdentity::for_launch("s1").unwrap())
        .unwrap();

    assert!(plan.enforced_by.contains(&(
        "read everything else: denied".to_owned(),
        "greywall Seatbelt (defaultDenyRead)".to_owned()
    )));
}

#[test]
fn a_profile_built_for_another_platform_is_refused() {
    let settings = tempfile::tempdir().unwrap();
    let profile = Profile::build(profile_request("/Users/owner/app"), &Platform::MacOs).unwrap();

    let refused = linux_engine(settings.path())
        .plan(&profile, &ClientIdentity::for_launch("s1").unwrap())
        .unwrap_err();

    assert_eq!(refused.rule, "platform macos");
}

#[test]
fn back_to_back_launches_of_one_session_get_distinct_proxy_clients() {
    let identities: HashSet<String> = (0..4)
        .map(|_| {
            ClientIdentity::for_launch("s1")
                .unwrap()
                .as_str()
                .to_owned()
        })
        .collect();

    assert_eq!(identities.len(), 4);
    for identity in &identities {
        assert!(identity.starts_with("cockpit-s1-"), "{identity}");
    }
}

#[test]
fn the_settings_file_is_owner_only_and_replaces_only_the_same_sessions_earlier_files() {
    let settings = tempfile::tempdir().unwrap();
    let engine = linux_engine(settings.path());
    let profile = Profile::build(profile_request("/home/owner/app"), &Platform::Linux).unwrap();
    let plan_for = |session: &str| {
        engine
            .plan(&profile, &ClientIdentity::for_launch(session).unwrap())
            .unwrap()
    };
    let earlier = plan_for("s1");
    let neighbour = plan_for("s1-2");
    earlier.write_settings().unwrap();
    neighbour.write_settings().unwrap();

    let current = plan_for("s1");
    current.write_settings().unwrap();

    let mode = std::fs::metadata(&current.settings_path)
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    assert_eq!(
        std::fs::read_to_string(&current.settings_path).unwrap(),
        current.settings
    );
    assert!(!earlier.settings_path.exists());
    assert!(neighbour.settings_path.exists());
}

/// A wrapper whose background child keeps the pipes open must not hold the daemon's start
/// past the deadline.
#[test]
fn detect_gives_up_on_a_program_that_leaves_its_output_open() {
    let bin = tempfile::tempdir().unwrap();
    let greywall = bin.path().join("greywall");
    std::fs::write(
        &greywall,
        "#!/bin/sh\necho 'Version: 0.3.7'\nsleep 60 &\nexit 0\n",
    )
    .unwrap();
    std::fs::set_permissions(&greywall, std::fs::Permissions::from_mode(0o755)).unwrap();
    let programs = GreywallPrograms {
        greywall,
        greyproxy: write_program(bin.path(), "greyproxy", GREYPROXY_VERSION_OUTPUT),
    };
    let started = std::time::Instant::now();

    let reason =
        Greywall::detect(programs, Platform::Linux, bin.path().join("settings")).unwrap_err();

    assert!(reason.contains("did not answer within"), "{reason}");
    assert!(started.elapsed() < std::time::Duration::from_secs(20));
}

fn words(plan: &Plan) -> Vec<String> {
    plan.arguments
        .iter()
        .map(|word| word.to_string_lossy().into_owned())
        .collect()
}

/// The values after each `flag` in `plan`'s arguments.
fn values_of(plan: &Plan, flag: &str) -> Vec<String> {
    let words = words(plan);
    words
        .windows(2)
        .filter(|pair| pair[0] == flag)
        .map(|pair| pair[1].clone())
        .collect()
}

fn plan_on_linux(request: ProfileRequest) -> Result<Plan, Refusal> {
    let settings = tempfile::tempdir().unwrap();
    let profile = Profile::build(request, &Platform::Linux)?;
    linux_engine(settings.path()).plan(&profile, &ClientIdentity::for_launch("s1").unwrap())
}

/// Greywall's private `/tmp` would hide Codex's socket folder from the daemon; the folder
/// is the host's, mounted over it (#437).
#[test]
fn a_codex_launch_mounts_the_session_directory_and_codexs_socket_folder() {
    let mut request = profile_request("/home/owner/app");
    request.provider = "codex".to_owned();

    let plan = plan_on_linux(request).unwrap();

    assert_eq!(
        values_of(&plan, "--allow-path"),
        ["/home/owner/app", SESSION_DIR, "/tmp/codex-daemon-1000"]
    );
    assert_eq!(plan.provider_folders, [Path::new("/tmp/codex-daemon-1000")]);
    assert!(plan.enforced_by.contains(&(
        "read-write /tmp/codex-daemon-1000 (codex's folder on the host, shared with the daemon)"
            .to_owned(),
        "greywall bubblewrap+Landlock (--allow-path, the host's own folder)".to_owned()
    )));
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

const FULL_DESKTOP: [(&str, &str); 5] = [
    ("XDG_RUNTIME_DIR", "/run/user/1000"),
    ("DISPLAY", ":0"),
    ("WAYLAND_DISPLAY", "wayland-1"),
    ("XAUTHORITY", "/run/user/1000/xauth_abc"),
    ("DBUS_SESSION_BUS_ADDRESS", "unix:path=/run/user/1000/bus"),
];

/// #435's acceptance: a shared desktop launch mounts the named display endpoints and nothing
/// else of `$XDG_RUNTIME_DIR` beyond Cockpit's own folders; the bus is greywall's filtered
/// proxy, never mounted from the host.
#[test]
fn a_shared_desktop_launch_mounts_the_named_display_endpoints_and_nothing_else_of_the_runtime_dir()
{
    let plan = plan_on_linux(shared_desktop(&FULL_DESKTOP)).unwrap();

    let mounted: Vec<String> = values_of(&plan, "--allow-path")
        .into_iter()
        .chain(values_of(&plan, "--allow-read-path"))
        .filter(|path| path.starts_with("/run/user/1000/") || path.starts_with("/tmp/"))
        .collect();
    assert_eq!(
        mounted,
        [
            SESSION_DIR,
            "/tmp/.X11-unix/X0",
            "/run/user/1000/wayland-1",
            "/run/user/1000/cockpit/agent",
            "/run/user/1000/xauth_abc",
        ]
    );
    assert!(plan.environment.contains(&(
        "DBUS_SESSION_BUS_ADDRESS".into(),
        "unix:path=/run/user/1000/bus".into()
    )));
}

#[test]
fn the_evidence_names_each_desktop_endpoint_and_how_it_is_reached() {
    let plan = plan_on_linux(shared_desktop(&FULL_DESKTOP)).unwrap();

    for (rule, enforced_by) in [
        (
            "desktop socket /tmp/.X11-unix/X0 for DISPLAY",
            "greywall bubblewrap+Landlock (--allow-path)",
        ),
        (
            "desktop socket /run/user/1000/wayland-1 for WAYLAND_DISPLAY",
            "greywall bubblewrap+Landlock (--allow-path)",
        ),
        (
            "desktop file /run/user/1000/xauth_abc for XAUTHORITY",
            "greywall bubblewrap+Landlock (--allow-read-path)",
        ),
        (
            "desktop bus /run/user/1000/bus for DBUS_SESSION_BUS_ADDRESS",
            "greywall's xdg-dbus-proxy in its place, org.freedesktop.Notifications only",
        ),
    ] {
        assert!(
            plan.enforced_by
                .contains(&(rule.to_owned(), enforced_by.to_owned())),
            "{rule} <- {enforced_by} missing from {:#?}",
            plan.enforced_by
        );
    }
}

/// The owner's own bus starts programs outside the sandbox: without greywall's filtered
/// proxy the bus is refused, never mounted.
#[test]
fn a_shared_bus_without_greywalls_proxy_is_refused() {
    let settings = tempfile::tempdir().unwrap();
    let available: Vec<&str> = LINUX_CAPABILITIES
        .into_iter()
        .filter(|name| *name != "xdg-dbus-proxy")
        .collect();
    let engine = engine_with(
        Platform::Linux,
        capabilities(&available, &["xdg-dbus-proxy"]),
        settings.path(),
    );
    let profile = Profile::build(shared_desktop(&FULL_DESKTOP), &Platform::Linux).unwrap();

    let refused = engine
        .plan(&profile, &ClientIdentity::for_launch("s1").unwrap())
        .unwrap_err();

    assert_eq!(
        refused.rule,
        "desktop bus /run/user/1000/bus for DBUS_SESSION_BUS_ADDRESS"
    );
    assert!(refused.reason.contains("xdg-dbus-proxy"), "{refused}");
}

/// Greywall mounts its proxy at `/run/user/<uid>/bus`; a bus elsewhere would leave the
/// variable pointing at nothing.
#[test]
fn a_shared_bus_other_than_the_owners_runtime_bus_is_refused() {
    let refused = plan_on_linux(shared_desktop(&[(
        "DBUS_SESSION_BUS_ADDRESS",
        "unix:path=/tmp/dbus-session",
    )]))
    .unwrap_err();

    assert_eq!(
        refused.rule,
        "desktop bus /tmp/dbus-session for DBUS_SESSION_BUS_ADDRESS"
    );
    assert!(refused.reason.contains("/run/user/1000/bus"), "{refused}");
}

#[test]
fn a_shared_bus_on_macos_is_refused() {
    let settings = tempfile::tempdir().unwrap();
    let engine = engine_with(Platform::MacOs, Vec::new(), settings.path());
    let request = shared_desktop(&[("DBUS_SESSION_BUS_ADDRESS", "unix:path=/run/user/1000/bus")]);
    let profile = Profile::build(request, &Platform::MacOs).unwrap();

    let refused = engine
        .plan(&profile, &ClientIdentity::for_launch("s1").unwrap())
        .unwrap_err();

    assert!(refused.reason.contains("only on Linux"), "{refused}");
}

fn plan_with_folder(folder: &Path, owner_uid: u32) -> Plan {
    let settings = tempfile::tempdir().unwrap();
    let profile = Profile::build(profile_request("/home/owner/app"), &Platform::Linux).unwrap();
    let mut plan = linux_engine(settings.path())
        .plan(&profile, &ClientIdentity::for_launch("s1").unwrap())
        .unwrap();
    plan.provider_folders = vec![folder.to_owned()];
    plan.owner_uid = owner_uid;
    plan
}

fn own_uid(dir: &Path) -> u32 {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(dir).unwrap().uid()
}

#[test]
fn a_missing_provider_folder_is_created_owner_only() {
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("codex-daemon-1000");

    plan_with_folder(&folder, own_uid(tmp.path()))
        .create_provider_folders()
        .unwrap();

    let mode = std::fs::metadata(&folder).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o700);
}

/// The folder is on the shared `/tmp`: one another user made first, or one others may open,
/// would hand them the agent's sockets.
#[test]
fn a_provider_folder_others_may_open_or_own_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let owner = own_uid(tmp.path());
    let open = tmp.path().join("open");
    std::fs::create_dir(&open).unwrap();
    std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o755)).unwrap();
    let elsewhere = tmp.path().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    let link = tmp.path().join("link");
    std::os::unix::fs::symlink(&elsewhere, &link).unwrap();
    let private = tmp.path().join("private");
    std::fs::create_dir(&private).unwrap();
    std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o700)).unwrap();

    for (folder, owner_uid) in [(&open, owner), (&link, owner), (&private, owner + 1)] {
        let (failed, error) = plan_with_folder(folder, owner_uid)
            .create_provider_folders()
            .unwrap_err();

        assert_eq!(&failed, folder);
        assert!(
            error.to_string().contains("only it may open"),
            "{}: {error}",
            folder.display()
        );
    }
    assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
}
