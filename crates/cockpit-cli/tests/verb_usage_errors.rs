//! A usage error prints the lines of the verb it came from plus a pointer to the whole list,
//! not the whole usage (#770). Driven as a process: every case fails while parsing, before any
//! connection to a daemon.

use std::process::{Command, Output};

const POINTER: &str = "`torlek` alone lists every verb";

/// One malformed command per verb family in the usage, with the family it must print.
const MISTAKES: &[(&[&str], &str)] = &[
    (&["config", "show", "--bogus"], "config"),
    (&["logs", "level", "--bogus", "x", "y"], "logs"),
    (&["agent", "report", "--state", "asleep"], "agent"),
    (&["bug-report", "new"], "bug-report"),
    (&["project", "add"], "project"),
    (&["checkout", "list", "a", "b", "c"], "checkout"),
    (&["status", "serve", "--bogus"], "status"),
    (&["storage", "prune"], "storage"),
    (&["usage", "bogus"], "usage"),
    (&["usage", "cost", "--days", "0"], "usage"),
    (&["ask", "run"], "ask"),
    (&["approval", "show"], "approval"),
    (&["guide", "show"], "guide"),
    (&["account", "sign-in", "codex"], "account"),
    (&["account", "grant", "app", "codex"], "account"),
    (&["account", "revoke", "first"], "account"),
    (&["network", "allow"], "network"),
    (&["session", "send"], "session"),
    (&["sandbox", "show"], "sandbox"),
    (&["provider", "bogus"], "provider"),
    (&["skills", "--all"], "skills"),
    (&["init", "--yes"], "init"),
    (&["doctor", "--bogus"], "doctor"),
    (&["tui", "--bogus"], "tui"),
    (&["notify", "--bogus"], "notify"),
    (&["notify", "away", "maybe"], "notify"),
    (&["update", "bogus"], "update"),
    (&["version", "--bogus"], "version"),
    (&["daemon", "install", "--bogus"], "daemon"),
    (&["todo", "add"], "todo"),
    (&["workflow", "show"], "workflow"),
    (&["check", "run"], "check"),
    (&["resources", "status", "x"], "resources"),
    (&["queue", "bogus"], "queue"),
    (&["desktop-session", "x"], "desktop-session"),
    (&["docs", "a", "b"], "docs"),
    (&["brief", "--bogus"], "brief"),
    (&["mcp", "x"], "mcp"),
    (&["--skill", "x"], "--skill"),
];

fn torlek(arguments: &[&str]) -> Output {
    let home = tempfile::tempdir().expect("temporary home");
    Command::new(env!("CARGO_BIN_EXE_torlek"))
        .args(["--profile", "torlek"])
        .args(arguments)
        .env_clear()
        .env("HOME", home.path())
        .output()
        .expect("torlek runs")
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("stderr is UTF-8")
}

#[test]
fn a_mistake_prints_only_its_verb_family_lines_and_the_pointer() {
    for (arguments, family) in MISTAKES {
        let output = torlek(arguments);
        let message = stderr(&output);

        assert_eq!(output.status.code(), Some(2), "{arguments:?}: {message}");
        let mut lines = message.lines();
        let problem = lines.next().expect("a problem line");
        assert!(problem.starts_with("torlek: "), "{arguments:?}: {message}");
        let usage_lines: Vec<&str> = lines.by_ref().take_while(|line| *line != POINTER).collect();
        assert!(!usage_lines.is_empty(), "{arguments:?}: {message}");
        assert!(lines.next().is_none(), "{arguments:?}: {message}");
        assert!(
            message.ends_with(&format!("{POINTER}\n")),
            "{arguments:?}: {message}"
        );
        let own_prefix = format!("torlek {family} ");
        for (position, line) in usage_lines.iter().enumerate() {
            let line = if position == 0 {
                line.strip_prefix("usage: ")
                    .expect("the first line starts the usage")
            } else {
                line.strip_prefix("       ")
                    .expect("continuation lines are indented")
            };
            assert!(
                format!("{line} ").starts_with(&own_prefix),
                "{arguments:?} printed another verb's line: {line}"
            );
        }
    }
}

#[test]
fn a_verb_family_error_is_much_shorter_than_the_whole_usage() {
    let whole_usage = stderr(&torlek(&["--help"]));
    let single_line_family = stderr(&torlek(&["doctor", "--bogus"]));

    assert!(whole_usage.len() > 4_000, "{whole_usage}");
    assert!(single_line_family.len() < 300, "{single_line_family}");
}

#[test]
fn an_unknown_verb_still_prints_the_whole_usage() {
    let message = stderr(&torlek(&["bogus"]));

    assert!(
        message.starts_with("usage: torlek [--profile NAME] VERB ...\n"),
        "{message}"
    );
    assert!(message.contains("\n       torlek todo add "), "{message}");
    assert!(
        message.contains("\n       torlek config show "),
        "{message}"
    );
    assert!(!message.contains(POINTER), "{message}");
}

#[test]
fn every_verb_family_of_the_usage_has_a_case_above() {
    let whole_usage = stderr(&torlek(&["--help"]));
    let families: std::collections::BTreeSet<&str> = whole_usage
        .lines()
        .skip(1)
        .filter_map(|line| line.split_whitespace().nth(1))
        .collect();
    let covered: std::collections::BTreeSet<&str> =
        MISTAKES.iter().map(|(_, family)| *family).collect();

    assert_eq!(families, covered);
}
