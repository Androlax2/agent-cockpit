//! What Claude Code declares it needs from the sandbox and where its credentials are (#830,
//! #858), read through the provider contract.

use std::path::PathBuf;

use cockpit_claude::ClaudeCode;
use cockpit_provider_api::{HomePath, Provider};

fn claude() -> ClaudeCode {
    ClaudeCode::new(PathBuf::from("/usr/bin/claude"), None, None)
}

fn strings<T: ToString>(values: &[T]) -> Vec<String> {
    values.iter().map(ToString::to_string).collect()
}

#[test]
fn it_reaches_anthropic_s_hosts_only() {
    let needs = claude().needs().unwrap();

    assert_eq!(
        strings(&needs.hosts),
        [
            "api.anthropic.com",
            "mcp-proxy.anthropic.com",
            "downloads.claude.ai",
            "platform.claude.com",
        ]
    );
}

#[test]
fn it_reads_only_its_installed_versions_from_the_owner_s_home() {
    assert_eq!(
        strings(&claude().needs().unwrap().read),
        ["~/.local/share/claude"]
    );
}

#[test]
fn its_configuration_folder_is_written_into_the_session_s_own() {
    let write = strings(&claude().needs().unwrap().write);

    for path in ["~/.claude", "~/.claude.json", "~/.config/claude"] {
        assert!(
            write.iter().any(|written| written == path),
            "{path} missing"
        );
    }
}

#[test]
fn it_names_its_credential_file_variables_and_host() {
    let credentials = claude().credentials().unwrap();

    assert_eq!(strings(&credentials.files), ["~/.claude/.credentials.json"]);
    let bindings: Vec<(&str, Vec<String>)> = credentials
        .bindings
        .iter()
        .map(|binding| (binding.variable.as_str(), strings(&binding.hosts)))
        .collect();
    assert_eq!(
        bindings,
        [
            ("ANTHROPIC_API_KEY", vec!["api.anthropic.com".to_owned()]),
            (
                "CLAUDE_CODE_OAUTH_TOKEN",
                vec!["api.anthropic.com".to_owned()]
            ),
        ]
    );
}

#[test]
fn its_credential_file_lies_in_a_folder_the_session_owns() {
    let needs = claude().needs().unwrap();
    let credential = HomePath::new("~/.claude/.credentials.json").unwrap();

    assert!(
        needs
            .write
            .iter()
            .any(|written| written.covers(&credential))
    );
    assert!(!needs.read.iter().any(|read| read.covers(&credential)));
}
