//! The launch gates (#1250) through `launch_gates` and the project report: one requirement
//! per gate, a fix for each that is not available, and the summary `doctor --project` shows.

use std::path::{Path, PathBuf};

use cockpit_config::{AgentSandbox, DesktopSession};
use cockpit_readiness::{
    HarnessTrust, LaunchFacts, Requirement, Status, Toolchains, launch_gates, launch_summary,
};

const PROFILE: &str = "sandbox: enforced by torlek-sandbox on linux";

/// A sandboxed launch of `provider` whose every gate passes.
fn passing(provider: &str) -> LaunchFacts {
    LaunchFacts {
        provider: provider.to_owned(),
        setting: AgentSandbox::Required,
        desktop: DesktopSession::Withheld,
        command: Ok(PathBuf::from(format!("/usr/bin/{provider}"))),
        backend: Ok("torlek-sandbox with bubblewrap 0.13.1".to_owned()),
        shared_desktop_refusal: None,
        account: Ok(Some("default".to_owned())),
        passed_over: Vec::new(),
        binds_sign_in: true,
        github: Ok(true),
        toolchains: Some(Ok(Toolchains {
            ecosystems: vec!["rust".to_owned()],
            install_roots: vec![PathBuf::from("/home/me/.cargo")],
        })),
        trust: HarnessTrust::Recorded {
            from: PathBuf::from("/home/me/app"),
        },
        profile: Some(Ok(PROFILE.to_owned())),
    }
}

fn gate<'gates>(gates: &'gates [Requirement], name: &str) -> &'gates Requirement {
    gates
        .iter()
        .find(|gate| gate.name == name)
        .unwrap_or_else(|| panic!("no {name} in {gates:#?}"))
}

#[test]
fn a_launch_whose_gates_pass_lists_each_gate_in_launch_order() {
    let gates = launch_gates(&passing("claude"));

    let names: Vec<&str> = gates.iter().map(|gate| gate.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "launch command",
            "launch backend",
            "launch desktop session",
            "launch sign-in",
            "launch github",
            "launch toolchains",
            "launch folder trust",
            "launch profile",
        ]
    );
    assert!(
        gates.iter().all(|gate| gate.status == Status::Available),
        "{gates:#?}"
    );
}

/// Each gate that is not available, with the status it gets and words its fix names.
#[test]
fn each_gate_that_does_not_pass_names_its_fix() {
    type Change = fn(&mut LaunchFacts);
    let cases: [(&str, Change, Status, &str); 9] = [
        (
            "launch command",
            |facts| facts.command = Err("effort \"x\" is not a level".to_owned()),
            Status::MissingPrerequisite,
            "`torlek doctor`",
        ),
        (
            "launch backend",
            |facts| facts.backend = Err("bubblewrap (`bwrap`) is not on PATH".to_owned()),
            Status::MissingPrerequisite,
            "agents.sandbox = \"off\"",
        ),
        (
            "launch desktop session",
            |facts| {
                facts.desktop = DesktopSession::Shared;
                facts.shared_desktop_refusal = Some("DISPLAY names /tmp/.X11-unix/X1".to_owned());
            },
            Status::MissingPrerequisite,
            "agents.desktop_session = \"withheld\"",
        ),
        (
            "launch sign-in",
            |facts| {
                facts.account = Err("account default: the credential file is a link".to_owned());
            },
            Status::MissingPrerequisite,
            "`torlek doctor`",
        ),
        (
            "launch sign-in",
            |facts| facts.account = Ok(None),
            Status::NeedsAuthorization,
            "the file to write",
        ),
        (
            "launch github",
            |facts| facts.github = Err("the token file is readable by others".to_owned()),
            Status::MissingPrerequisite,
            "token file",
        ),
        (
            "launch toolchains",
            |facts| facts.toolchains = Some(Err("rustup's root is a link".to_owned())),
            Status::MissingPrerequisite,
            "PATH",
        ),
        (
            "launch folder trust",
            |facts| {
                facts.trust = HarnessTrust::Refused {
                    reason: "its project /home/me/app is not in projects.trusted".to_owned(),
                };
            },
            Status::NeedsAuthorization,
            "projects.trusted",
        ),
        (
            "launch profile",
            |facts| facts.profile = Some(Err("launch spec: no HOME".to_owned())),
            Status::MissingPrerequisite,
            "report it",
        ),
    ];
    for (name, change, status, fix) in cases {
        let mut facts = passing("claude");
        change(&mut facts);

        let gates = launch_gates(&facts);

        let found = gate(&gates, name);
        assert_eq!(found.status, status, "{found:#?}");
        assert!(
            found.next.as_deref().is_some_and(|next| next.contains(fix)),
            "{found:#?}"
        );
    }
}

#[test]
fn an_unsandboxed_launch_shares_the_desktop_without_refusing_and_reads_no_toolchain() {
    let mut facts = passing("claude");
    facts.setting = AgentSandbox::Off;
    facts.desktop = DesktopSession::Shared;
    facts.shared_desktop_refusal = Some("DISPLAY names /tmp/.X11-unix/X1".to_owned());
    facts.account = Ok(None);
    facts.binds_sign_in = false;
    facts.toolchains = None;

    let gates = launch_gates(&facts);

    assert!(
        gates.iter().all(|gate| gate.status == Status::Available),
        "{gates:#?}"
    );
    assert!(
        gate(&gates, "launch backend")
            .detail
            .contains("UNSANDBOXED"),
        "{gates:#?}"
    );
}

#[test]
fn a_profile_not_built_is_unknown_and_points_at_the_gates_above() {
    let mut facts = passing("claude");
    facts.backend = Err("bubblewrap (`bwrap`) is not on PATH".to_owned());
    facts.toolchains = None;
    facts.profile = None;

    let gates = launch_gates(&facts);

    assert_eq!(gate(&gates, "launch profile").status, Status::Unknown);
    assert_eq!(gate(&gates, "launch toolchains").status, Status::Unknown);
}

#[test]
fn a_summary_of_passing_launches_is_one_line_naming_each_provider() {
    let summary = launch_summary(
        &[passing("claude"), passing("codex")],
        Path::new("/home/me/app"),
    );

    assert_eq!(summary.len(), 1, "{summary:#?}");
    assert_eq!(summary[0].name, "launch");
    assert_eq!(summary[0].status, Status::Available);
    assert!(
        summary[0]
            .detail
            .starts_with("a session of claude, codex in /home/me/app passes every launch gate"),
        "{summary:#?}"
    );
}

/// A host's gate is listed once; a provider's names its provider, each time; a profile the refusal
/// kept from being built is left out.
#[test]
fn a_summary_of_several_providers_lists_each_open_gate_once() {
    let refused = |provider: &str| {
        let mut facts = passing(provider);
        facts.backend = Err("bubblewrap (`bwrap`) is not on PATH".to_owned());
        facts.toolchains = None;
        facts.profile = None;
        facts.account = Ok(None);
        facts
    };

    let summary = launch_summary(
        &[refused("claude"), refused("codex")],
        Path::new("/home/me/app"),
    );

    let names: Vec<&str> = summary.iter().map(|gate| gate.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "launch backend",
            "launch claude sign-in",
            "launch claude toolchains",
            "launch codex sign-in",
            "launch codex toolchains",
        ]
    );
}

#[test]
fn a_summary_without_a_provider_is_unknown_rather_than_passing() {
    let summary = launch_summary(&[], Path::new("/home/me/app"));

    assert_eq!(summary.len(), 1, "{summary:#?}");
    assert_eq!(summary[0].name, "launch");
    assert_eq!(summary[0].status, Status::Unknown);
}
