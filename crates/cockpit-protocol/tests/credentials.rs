//! The wire form of the owner's credential commands (#1251): a value goes in a request once,
//! never in a reply, and formatting a request never shows it.
use cockpit_protocol::{
    Command, CredentialFileView, CredentialStateView, CredentialTarget, CredentialView, Reply,
    SecretText, decode_request,
};
use serde_json::json;

const SECRET: &str = "sk-ant-oat01-fixture";

#[test]
fn each_credential_command_has_its_kebab_case_kind() {
    let provider = CredentialTarget::Provider {
        provider: "claude".to_owned(),
        variable: Some("CLAUDE_CODE_OAUTH_TOKEN".to_owned()),
    };
    let api_token = CredentialTarget::GithubToken {
        project: "app".to_owned(),
        api: true,
    };
    let cases = [
        (
            json!({"kind": "credential-set", "value": SECRET, "target": {
                "kind": "provider", "provider": "claude", "variable": "CLAUDE_CODE_OAUTH_TOKEN"}}),
            Command::CredentialSet {
                target: provider.clone(),
                value: SecretText::new(SECRET.to_owned()),
            },
        ),
        (
            json!({"kind": "credential-show", "target": {
                "kind": "github-token", "project": "app", "api": true}}),
            Command::CredentialShow {
                target: api_token.clone(),
            },
        ),
        (
            json!({"kind": "credential-clear", "target": {
                "kind": "provider", "provider": "claude"}}),
            Command::CredentialClear {
                target: CredentialTarget::Provider {
                    provider: "claude".to_owned(),
                    variable: None,
                },
            },
        ),
        (
            json!({"kind": "credential-show", "target": {"kind": "github-token", "project": "app"}}),
            Command::CredentialShow {
                target: CredentialTarget::GithubToken {
                    project: "app".to_owned(),
                    api: false,
                },
            },
        ),
    ];

    for (command, expected) in cases {
        let wire = json!({"id": 1, "command": command});
        assert_eq!(
            decode_request(&wire.to_string()).unwrap().command,
            expected,
            "{wire}"
        );
        assert_eq!(serde_json::to_value(&expected).unwrap(), command);
    }
}

#[test]
fn formatting_a_set_request_never_shows_its_value() {
    let command = Command::CredentialSet {
        target: CredentialTarget::GithubToken {
            project: "app".to_owned(),
            api: false,
        },
        value: SecretText::new(SECRET.to_owned()),
    };

    let formatted = format!("{command:?} {command:#?}");

    assert!(!formatted.contains(SECRET), "{formatted}");
    assert!(formatted.contains("[redacted]"), "{formatted}");
}

#[test]
fn a_credential_reply_round_trips_with_paths_and_modes_alone() {
    let reply = Reply::Credential {
        credential: CredentialView {
            files: vec![
                CredentialFileView {
                    path: "/c/ANTHROPIC_API_KEY".to_owned(),
                    mode: Some(0o600),
                },
                CredentialFileView {
                    path: "/c/CLAUDE_CODE_OAUTH_TOKEN".to_owned(),
                    mode: None,
                },
            ],
            state: CredentialStateView::Usable {
                path: "/c/ANTHROPIC_API_KEY".to_owned(),
            },
        },
    };

    let wire = serde_json::to_value(&reply).unwrap();

    assert_eq!(
        wire,
        json!({"kind": "credential", "credential": {
            "files": [
                {"path": "/c/ANTHROPIC_API_KEY", "mode": 384},
                {"path": "/c/CLAUDE_CODE_OAUTH_TOKEN"}
            ],
            "state": {"kind": "usable", "path": "/c/ANTHROPIC_API_KEY"}
        }})
    );
    assert_eq!(serde_json::from_value::<Reply>(wire).unwrap(), reply);
}
