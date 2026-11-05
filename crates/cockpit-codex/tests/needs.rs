//! What Codex declares it needs from the sandbox and where its credentials are (#830, #858),
//! read through the provider contract.

use std::path::PathBuf;

use cockpit_codex::Codex;
use cockpit_provider_api::{HomePath, Provider};

fn codex() -> Codex {
    Codex::new(PathBuf::from("/usr/bin/codex"), None, None)
}

fn strings<T: ToString>(values: &[T]) -> Vec<String> {
    values.iter().map(ToString::to_string).collect()
}

#[test]
fn it_reaches_openai_s_and_chatgpt_s_hosts_only() {
    assert_eq!(
        strings(&codex().needs().unwrap().hosts),
        [
            "api.openai.com",
            "chatgpt.com",
            "ab.chatgpt.com",
            "auth.openai.com"
        ]
    );
}

#[test]
fn it_reads_nothing_from_the_owner_s_home_and_writes_its_own_folders() {
    let needs = codex().needs().unwrap();

    assert!(needs.read.is_empty());
    assert_eq!(strings(&needs.write), ["~/.codex", "~/.cache/codex"]);
}

#[test]
fn it_names_its_credential_file_variables_and_host() {
    let credentials = codex().credentials().unwrap();

    assert_eq!(strings(&credentials.files), ["~/.codex/auth.json"]);
    let bindings: Vec<(&str, Vec<String>)> = credentials
        .bindings
        .iter()
        .map(|binding| (binding.variable.as_str(), strings(&binding.hosts)))
        .collect();
    assert_eq!(
        bindings,
        [
            ("OPENAI_API_KEY", vec!["api.openai.com".to_owned()]),
            ("CODEX_API_KEY", vec!["api.openai.com".to_owned()]),
        ]
    );
}

#[test]
fn its_credential_file_lies_in_a_folder_the_session_owns() {
    let needs = codex().needs().unwrap();
    let credential = HomePath::new("~/.codex/auth.json").unwrap();

    assert!(
        needs
            .write
            .iter()
            .any(|written| written.covers(&credential))
    );
}
