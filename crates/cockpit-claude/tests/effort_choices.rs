//! The effort levels come from the installed Claude Code's `--help`, never from a list in
//! the adapter (#743).

mod common;

use cockpit_claude::ClaudeCode;
use cockpit_provider_api::Provider;

/// A command that prints the `--effort` paragraph of `claude --help` as 2.1.319 words it,
/// with `levels`.
fn help_listing(dir: &std::path::Path, levels: &str) -> String {
    let help = format!(
        "Options:\n  --debug              Enable debug mode\n  \
         --effort <level>     Effort level for the current session\n{:38}({levels})\n  \
         --environment <environment_id>  Create a new cloud session\n",
        ""
    );
    let file = dir.join("help.txt");
    std::fs::write(&file, help).unwrap();
    format!("cat '{}'", file.display())
}

fn claude_whose_help_is(dir: &std::path::Path, help: &str) -> ClaudeCode {
    let body = format!(
        "if [ \"$1\" = --version ]; then echo '9.9.9 (Claude Code)'; exit 0; fi\n\
         if [ \"$1\" = --help ]; then {help}; exit 0; fi\nexit 64"
    );
    let program = common::script(dir, "claude", &body);
    ClaudeCode::discover(&program, None).unwrap()
}

#[test]
fn a_level_a_claude_code_update_adds_is_accepted_without_a_torlek_release() {
    let dir = tempfile::tempdir().unwrap();
    let claude = claude_whose_help_is(
        dir.path(),
        &help_listing(dir.path(), "low, medium, high, ludicrous"),
    );

    assert_eq!(claude.check_choices(None, Some("ludicrous")), Ok(()));
}

#[test]
fn a_level_the_installed_claude_code_does_not_list_is_refused_naming_its_levels_and_version() {
    let dir = tempfile::tempdir().unwrap();
    let claude = claude_whose_help_is(dir.path(), &help_listing(dir.path(), "low, medium, high"));

    let error = claude.check_choices(None, Some("xhigh")).unwrap_err();

    assert_eq!(
        error.reason,
        "effort \"xhigh\" is not one of low, medium, high, the levels `claude --help` lists \
         (Claude Code 9.9.9)"
    );
}

#[test]
fn a_level_a_claude_code_update_removes_is_refused_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let claude = claude_whose_help_is(dir.path(), &help_listing(dir.path(), "low, medium"));

    assert!(claude.check_choices(None, Some("max")).is_err());
}

#[test]
fn a_list_that_wraps_onto_the_next_line_is_read() {
    let dir = tempfile::tempdir().unwrap();
    let help = "printf '  --effort <level>   Effort level for the current\\n                     session (low,\\n  medium, high)\\n  --next  x\\n'";
    let claude = claude_whose_help_is(dir.path(), help);

    assert_eq!(claude.effort_levels(), ["low", "medium", "high"]);
}

#[test]
fn a_help_that_lists_no_levels_passes_the_choice_through_for_claude_code_to_refuse() {
    for help in [
        "echo 'Usage: claude [options]'",
        "echo '  --effort <level>  Effort for the session'",
        "echo '  --effort <level>  Effort (see the docs, ok?)'",
        "exit 3",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let claude = claude_whose_help_is(dir.path(), help);

        assert_eq!(
            claude.check_choices(None, Some("anything")),
            Ok(()),
            "{help}"
        );
    }
}

#[test]
fn a_start_without_an_effort_does_not_ask_claude_code_anything() {
    let dir = tempfile::tempdir().unwrap();
    let claude = claude_whose_help_is(dir.path(), "exit 3");
    std::fs::remove_file(dir.path().join("claude")).unwrap();

    assert_eq!(claude.check_choices(Some("opus"), None), Ok(()));
}

#[test]
fn the_help_is_read_once_per_adapter_and_so_per_harness_version() {
    let dir = tempfile::tempdir().unwrap();
    let claude = claude_whose_help_is(dir.path(), &help_listing(dir.path(), "low, high"));
    assert_eq!(claude.effort_levels(), ["low", "high"]);
    std::fs::remove_file(dir.path().join("claude")).unwrap();

    assert!(claude.check_choices(None, Some("medium")).is_err());
}

#[test]
fn a_model_is_never_checked_against_a_list() {
    let dir = tempfile::tempdir().unwrap();
    let claude = claude_whose_help_is(dir.path(), &help_listing(dir.path(), "low"));

    assert_eq!(
        claude.check_choices(Some("a-model-released-today"), None),
        Ok(())
    );
}
