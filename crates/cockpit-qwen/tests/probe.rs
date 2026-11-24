//! The runtime probe of a version not on the tested list (#732), with fake programs.

mod common;

use std::path::Path;

use cockpit_provider_api::Capability;

/// `qwen --help` as 0.28.4's yargs options word it, without `without`.
fn help_without(without: Option<&str>) -> String {
    [
        "  -m, --model                 Model",
        "  -i, --prompt-interactive    Execute the provided prompt and continue in interactive mode",
        "  -r, --resume                Resume a specific session by its ID.",
        "      --session-id            Specify a session ID for this run.",
        "      --session-id-file       Not a flag the adapter passes.",
    ]
    .into_iter()
    .filter(|line| without.is_none_or(|flag| !line.contains(&format!("{flag} "))))
    .collect::<Vec<_>>()
    .join("\n")
}

fn fake_qwen(dir: &Path, help: &str) -> std::path::PathBuf {
    common::script(dir, "qwen", &format!("cat <<'HELP'\n{help}\nHELP"))
}

#[test]
fn a_qwen_documenting_every_launch_flag_probes_clean() {
    let dir = tempfile::tempdir().unwrap();
    let program = fake_qwen(dir.path(), &help_without(None));

    let probe = cockpit_qwen::probe(&program, "0.29.0").unwrap();

    assert!(probe.is_clean(), "{}", probe.summary());
}

#[test]
fn a_missing_launch_flag_is_named_and_takes_every_capability() {
    let dir = tempfile::tempdir().unwrap();
    let program = fake_qwen(dir.path(), &help_without(Some("--session-id")));

    let probe = cockpit_qwen::probe(&program, "0.29.0").unwrap();

    assert_eq!(probe.missing().len(), 1);
    assert_eq!(
        probe.missing()[0].requirement,
        "flag --session-id in `qwen --help`"
    );
    assert!(probe.blocks(Capability::Resume));
    assert!(probe.blocks(Capability::Events));
}

#[test]
fn a_qwen_that_cannot_run_is_an_error_not_a_finding() {
    assert!(cockpit_qwen::probe(Path::new("/nonexistent/qwen"), "0.29.0").is_err());
}
