//! Issue #729: inside a session `torlek` sends the owner's verbs on the agent socket, where
//! the real daemon refuses them, even when the host-only socket is reachable as it is for an
//! agent launched with `agents.sandbox = "off"`; from the owner's own shell they reach the
//! host-only socket as before.
mod common;

use std::path::Path;
use std::process::Output;

use common::TestHome;

/// What a launch puts in its agent's environment.
const SESSION_ENVIRONMENT: [(&str, &str); 2] = [
    ("TORLEK_SESSION", "s0123456789ab"),
    ("TORLEK_LAUNCH_TOKEN", "the-token"),
];

/// Owner verbs whose refusal the issue names, each as `torlek` arguments; `DIR` stands for a
/// registrable project.
const OWNER_VERBS: [&[&str]; 4] = [
    &["project", "add", "DIR"],
    &["session", "end", "s0123456789ab"],
    &["session", "end", "--finished"],
    &["session", "grant", "s0123456789ab", "start-sessions"],
];

fn arguments<'word>(verb: &[&'word str], project: &'word str) -> Vec<&'word str> {
    verb.iter()
        .map(|word| if *word == "DIR" { project } else { word })
        .collect()
}

fn torlek(home: &TestHome, arguments: &[&str], environment: &[(&str, &str)]) -> Output {
    home.base_command(Path::new(env!("CARGO_BIN_EXE_torlek")))
        .args(arguments)
        .envs(environment.iter().copied())
        .output()
        .expect("the torlek binary runs")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn inside_a_session_each_owner_verb_is_refused_on_the_agent_socket() {
    let home = TestHome::new();
    let _daemon = home.start();
    let project = home.project("app");
    let project = project.to_str().unwrap();

    for verb in OWNER_VERBS {
        let output = torlek(&home, &arguments(verb, project), &SESSION_ENVIRONMENT);

        let stderr = text(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "{verb:?}: {stderr}");
        assert!(
            stderr.contains("not-authorized: this command is the owner's alone"),
            "{verb:?}: {stderr}"
        );
        assert!(
            stderr.contains(
                "to unblock (the owner): the owner runs it from their own terminal, \
                 not from a session"
            ),
            "{verb:?}: {stderr}"
        );
    }
    assert_eq!(home.cockpit_ok(&["project", "list"]), "");
}

#[test]
fn a_session_known_only_by_its_launch_token_is_refused_too() {
    let home = TestHome::new();
    let _daemon = home.start();
    let project = home.project("app");

    let output = torlek(
        &home,
        &["project", "add", project.to_str().unwrap()],
        &[("TORLEK_LAUNCH_TOKEN", "the-token")],
    );

    let stderr = text(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("this command is the owner's alone"),
        "{stderr}"
    );
    assert_eq!(home.cockpit_ok(&["project", "list"]), "");
}

#[test]
fn from_the_owners_shell_each_owner_verb_reaches_the_host_only_socket() {
    let home = TestHome::new();
    let _daemon = home.start();
    let project = home.project("app");
    let project = project.to_str().unwrap();

    for verb in OWNER_VERBS {
        let output = torlek(&home, &arguments(verb, project), &[]);

        let stderr = text(&output.stderr);
        assert!(
            !stderr.contains("owner's alone"),
            "{verb:?} was refused as an agent's: {stderr}"
        );
    }
    assert!(
        home.cockpit_ok(&["project", "list"]).contains(project),
        "the owner's project add registered nothing"
    );
}
