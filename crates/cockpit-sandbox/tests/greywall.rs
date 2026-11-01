//! The Greywall backend: the pinned builds it accepts, the host capabilities it requires,
//! the command and settings file it plans for a profile, and the per-launch greyproxy client
//! identity that keeps back-to-back launches from replacing each other's client secret.

mod common;

use std::collections::HashSet;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use cockpit_sandbox::{
    ClientIdentity, EngineBuild, Greywall, GreywallPrograms, PINNED_GREYPROXY_COMMIT,
    PINNED_GREYWALL_COMMIT, Platform, Profile, parse_greyproxy_version, parse_greywall_version,
};
use common::{
    capabilities, engine_with, linux_engine, pinned_greyproxy, pinned_greywall, profile_request,
    programs,
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
