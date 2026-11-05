//! What OpenCode declares it needs from the sandbox and where its credentials are (#830,
//! #858), read through the provider contract.

use std::path::PathBuf;

use cockpit_opencode::OpenCode;
use cockpit_provider_api::{HomePath, Provider};

fn opencode() -> OpenCode {
    OpenCode::new(PathBuf::from("/usr/bin/opencode"), None, None)
}

fn strings<T: ToString>(values: &[T]) -> Vec<String> {
    values.iter().map(ToString::to_string).collect()
}

#[test]
fn it_reaches_the_two_model_apis_and_the_model_catalog_only() {
    assert_eq!(
        strings(&opencode().needs().unwrap().hosts),
        ["api.openai.com", "api.anthropic.com", "models.dev"]
    );
}

#[test]
fn it_reads_only_its_install_folder_from_the_owner_s_home() {
    assert_eq!(strings(&opencode().needs().unwrap().read), ["~/.opencode"]);
}

#[test]
fn it_names_its_credential_files_variables_and_hosts() {
    let credentials = opencode().credentials().unwrap();

    assert_eq!(
        strings(&credentials.files),
        [
            "~/.local/share/opencode/auth.json",
            "~/.local/share/opencode/account.json",
            "~/.config/github-copilot",
        ]
    );
    let bindings: Vec<(&str, Vec<String>)> = credentials
        .bindings
        .iter()
        .map(|binding| (binding.variable.as_str(), strings(&binding.hosts)))
        .collect();
    assert_eq!(
        bindings,
        [
            ("ANTHROPIC_API_KEY", vec!["api.anthropic.com".to_owned()]),
            ("OPENAI_API_KEY", vec!["api.openai.com".to_owned()]),
        ]
    );
}

/// The Copilot sign-in is the owner's and no folder of the session's backs it: it is neither
/// read from the home nor written.
#[test]
fn no_declared_path_exposes_a_credential_of_the_owner_s() {
    let needs = opencode().needs().unwrap();
    let copilot = HomePath::new("~/.config/github-copilot").unwrap();

    assert!(!needs.read.iter().any(|read| read.covers(&copilot)));
    assert!(!needs.write.iter().any(|written| written.covers(&copilot)));
}
