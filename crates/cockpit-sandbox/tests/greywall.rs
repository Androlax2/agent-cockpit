//! The Greywall backend: the pinned builds it accepts, the host capabilities it requires,
//! the command and settings file it plans for a profile, and the per-launch greyproxy client
//! identity that keeps back-to-back launches from replacing each other's client secret.

mod common;

use std::collections::HashSet;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use cockpit_sandbox::{
    ClientIdentity, DesktopSession, EngineBuild, FolderMount, Greywall, GreywallPrograms,
    MOUNT_FLAG, PINNED_GREYPROXY_COMMIT, PINNED_GREYWALL_COMMIT, Plan, Platform, Profile,
    ProfileRequest, ProviderReach, Refusal, lists_flag, parse_greyproxy_version,
    parse_greywall_version,
};
use common::{
    LINUX_CAPABILITIES, MOUNT_SOURCES, SESSION_DIR, capabilities, engine_with, linux_engine,
    pinned_greyproxy, pinned_greywall, profile_request, programs,
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

/// A stand-in greywall whose `--help` prints `help` and anything else prints its version.
fn write_greywall(dir: &Path, help: &str) -> std::path::PathBuf {
    let path = dir.join("greywall");
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\nif [ \"$1\" = --help ]; then\ncat <<'EOF'\n{help}EOF\nexit 0\nfi\ncat <<'EOF'\n{GREYWALL_VERSION_OUTPUT}EOF\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// `greywall --help` of the installed build without the fork's `--mount` (excerpt).
const HELP_WITHOUT_MOUNT: &str = "Usage:
  greywall [flags] -- [command...]

Examples:
  greywall --allow-path /tmp/work -- mytool      # Grant rw to an extra dir/file for this run

Flags:
      --allow-path stringArray        Allow read+write access to a directory or file for this session (repeatable, e.g. --allow-path /tmp/work)
      --allow-read-path stringArray   Allow read-only access to a directory or file for this session (repeatable, e.g. --allow-read-path /data/refs)
  -s, --settings string               Path to settings file (default: OS config directory)
";

/// The same with the fork's `--mount` (#446).
const HELP_WITH_MOUNT: &str = "Usage:
  greywall [flags] -- [command...]

Flags:
      --allow-path stringArray        Allow read+write access to a directory or file for this session (repeatable, e.g. --allow-path /tmp/work)
      --mount stringArray             Mount a host directory at a path below /tmp inside the sandbox (repeatable, Linux only, e.g. --mount /srv/x:/tmp/x)
  -s, --settings string               Path to settings file (default: OS config directory)
";

#[test]
fn detect_finds_mount_when_greywalls_help_lists_it() {
    for (help, can_mount_folders) in [(HELP_WITH_MOUNT, true), (HELP_WITHOUT_MOUNT, false)] {
        let bin = tempfile::tempdir().unwrap();
        let programs = GreywallPrograms {
            greywall: write_greywall(bin.path(), help),
            greyproxy: write_program(bin.path(), "greyproxy", GREYPROXY_VERSION_OUTPUT),
        };

        let engine =
            Greywall::detect(programs, Platform::Linux, bin.path().join("settings")).unwrap();

        assert_eq!(engine.can_mount_folders, can_mount_folders, "{help}");
    }
}

#[test]
fn only_an_option_line_lists_a_flag() {
    assert!(lists_flag(HELP_WITH_MOUNT, "--mount"));
    assert!(lists_flag(HELP_WITH_MOUNT, "--settings"));
    for help in [
        "  greywall --mount /srv/x:/tmp/x -- tool   # an example\n",
        "      --allow-path stringArray   e.g. --mount is not this line's flag\n",
        "      --mount-all   another flag\n",
    ] {
        assert!(!lists_flag(help, "--mount"), "{help}");
    }
}

/// Greywall refuses a macOS configuration with mounts: a Codex launch there goes ahead with
/// its sockets unreachable.
#[test]
fn a_codex_launch_on_macos_mounts_nothing() {
    let settings = tempfile::tempdir().unwrap();
    let mut engine = engine_with(Platform::MacOs, Vec::new(), settings.path());
    engine.can_mount_folders = true;
    let mut request = profile_request("/home/owner/app");
    request.provider = "codex".to_owned();
    let profile = Profile::build(request, &Platform::MacOs).unwrap();

    let plan = engine
        .plan(&profile, &ClientIdentity::for_launch("s1").unwrap())
        .unwrap();

    assert!(values_of(&plan, "--mount").is_empty());
    let ProviderReach::Unreachable { reason, .. } = &plan.reach else {
        panic!("{:?}", plan.reach);
    };
    assert!(reason.contains("only on Linux"), "{reason}");
    let folder_evidence = plan
        .enforced_by
        .iter()
        .find(|(rule, _)| rule.starts_with("folder "))
        .map(|(_, evidence)| evidence.as_str());
    assert!(
        folder_evidence.is_some_and(|evidence| evidence.contains("only on Linux")),
        "{folder_evidence:?}"
    );
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

fn codex_plan(can_mount_folders: bool, settings: &Path) -> Plan {
    let mut request = profile_request("/home/owner/app");
    request.provider = "codex".to_owned();
    let profile = Profile::build(request, &Platform::Linux).unwrap();
    let mut engine = linux_engine(settings);
    engine.can_mount_folders = can_mount_folders;
    engine
        .plan(&profile, &ClientIdentity::for_launch("s1").unwrap())
        .unwrap()
}

/// A greywall whose `--help` lists `--mount` shows the session's own folder at Codex's
/// socket path, and the daemon reaches Codex's sockets through it (#446).
#[test]
fn a_codex_launch_mounts_its_sessions_own_folder_at_codexs_socket_path() {
    let settings = tempfile::tempdir().unwrap();

    let plan = codex_plan(true, settings.path());

    let host = format!("{MOUNT_SOURCES}/s1/codex-daemon");
    assert_eq!(
        values_of(&plan, "--mount"),
        [format!("{host}:/tmp/codex-daemon-1000")]
    );
    assert_eq!(
        plan.reach,
        ProviderReach::Mounted(vec![FolderMount {
            host: PathBuf::from(&host),
            sandbox: PathBuf::from("/tmp/codex-daemon-1000"),
        }])
    );
    assert!(plan.enforced_by.contains(&(
        format!("folder {host} at /tmp/codex-daemon-1000 (codex's own, not the host's)"),
        "greywall --mount".to_owned()
    )));
}

/// Without `--mount` the launch goes ahead with a private `/tmp`, and delivery is
/// refused naming the feature: the host's folder is never shared instead (#446).
#[test]
fn without_mount_a_codex_launch_runs_but_its_sockets_are_unreachable() {
    let settings = tempfile::tempdir().unwrap();

    let plan = codex_plan(false, settings.path());

    assert!(values_of(&plan, "--mount").is_empty());
    let ProviderReach::Unreachable { feature, reason } = &plan.reach else {
        panic!("{:?}", plan.reach);
    };
    assert_eq!(*feature, MOUNT_FLAG);
    assert!(reason.contains("--help lists no --mount"), "{reason}");
}

#[test]
fn no_codex_launch_shares_the_hosts_codex_socket_folder() {
    let settings = tempfile::tempdir().unwrap();
    for can_mount_folders in [true, false] {
        let plan = codex_plan(can_mount_folders, settings.path());

        assert_eq!(
            values_of(&plan, "--allow-path"),
            ["/home/owner/app", SESSION_DIR]
        );
    }
}

/// Greywall's mount checks its source and then mounts it: the source and the folder holding
/// it are the daemon's, outside every path the sandbox lets the agent write (#446).
#[test]
fn the_mount_source_is_outside_every_path_the_sandbox_writes() {
    let settings = tempfile::tempdir().unwrap();

    let plan = codex_plan(true, settings.path());

    let writable = values_of(&plan, "--allow-path");
    let sources = values_of(&plan, "--mount");
    assert!(!sources.is_empty());
    for source in sources {
        let host = Path::new(source.split_once(':').unwrap().0);
        let holder = host.parent().unwrap();
        for granted in &writable {
            assert!(
                !holder.starts_with(granted) && !Path::new(granted).starts_with(holder),
                "{} is inside or holds {granted}",
                host.display()
            );
        }
    }
}

fn mounted_plan(host: &Path) -> Plan {
    let settings = tempfile::tempdir().unwrap();
    let mut plan = codex_plan(true, settings.path());
    plan.reach = ProviderReach::Mounted(vec![FolderMount {
        host: host.to_owned(),
        sandbox: PathBuf::from("/tmp/codex-daemon-1000"),
    }]);
    plan
}

#[test]
fn a_mounted_folder_is_created_owner_only_in_the_daemons_mount_sources() {
    let mount_sources = tempfile::tempdir().unwrap();
    let folder = mount_sources.path().join("s1/codex-daemon");

    mounted_plan(&folder).create_mount_sources().unwrap();

    for created in [folder.parent().unwrap(), &folder] {
        let mode = std::fs::metadata(created).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700, "{}", created.display());
    }
}

/// A link where a mounted folder belongs would mount whatever it names: never followed.
#[test]
fn a_link_in_place_of_the_mounted_folder_is_refused() {
    let mount_sources = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    std::fs::create_dir(mount_sources.path().join("s1")).unwrap();
    let folder = mount_sources.path().join("s1/codex-daemon");
    std::os::unix::fs::symlink(elsewhere.path(), &folder).unwrap();

    let (failed, error) = mounted_plan(&folder).create_mount_sources().unwrap_err();

    assert_eq!(failed, folder);
    assert!(
        error.to_string().contains("is not a folder of user"),
        "{error}"
    );
    assert!(std::fs::symlink_metadata(&folder).unwrap().is_symlink());
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
        .filter(|name| *name != "D-Bus isolated")
        .collect();
    let engine = engine_with(
        Platform::Linux,
        capabilities(&available, &["D-Bus isolated"]),
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
    assert!(refused.reason.contains("D-Bus isolated"), "{refused}");
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
