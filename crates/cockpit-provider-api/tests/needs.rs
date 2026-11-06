//! What a program declares it needs from the sandbox, validated where it is made (#830, #861).

use cockpit_provider_api::{CredentialHeader, Credentials, HomePath, Host, Needs, Provider};

#[test]
fn a_host_is_a_lower_case_name_or_a_domain_wildcard_with_an_optional_port() {
    for host in [
        "api.anthropic.com",
        "api.anthropic.com:443",
        "*.githubusercontent.com",
        "xn--bcher-kva.example",
        "a-b.example.com:65535",
    ] {
        assert_eq!(Host::new(host).unwrap().as_str(), host);
    }
}

#[test]
fn a_host_outside_its_rule_is_refused() {
    for host in [
        "",
        "*",
        "*:443",
        "*.com",
        "ex*ample.com",
        "example.*",
        "API.example.com",
        "127.0.0.1",
        "0x7f000001",
        "127.0.0.0x1",
        "2130706433",
        "localhost",
        "localhost:5432",
        "db.localhost",
        "intranet",
        "10.0.0.1:443",
        "[::1]",
        "::1",
        "example.com:0",
        "example.com:http",
        "example.com:+443",
        "example.com:65536",
        "-example.com",
        "example-.com",
        "example..com",
        "example.com.",
        &format!("{}.com", "a".repeat(64)),
    ] {
        assert!(Host::new(host).is_err(), "{host:?} was admitted");
    }
}

#[test]
fn a_home_path_is_tilde_slash_and_names_with_a_wildcard_only_in_the_last() {
    for path in [
        "~/.claude",
        "~/.claude.json.*",
        "~/.local/share/claude",
        "~/.nvm/versions/node/v*",
    ] {
        assert_eq!(HomePath::new(path).unwrap().as_str(), path);
    }
}

#[test]
fn a_home_path_outside_its_rule_is_refused() {
    for path in [
        "",
        "~",
        "~/",
        "/home/owner/.claude",
        ".claude",
        "~/.claude/",
        "~//x",
        "~/../etc",
        "~/a/./b",
        "~/*",
        "~/.*",
        "~/..*",
        "~/**",
        "~/*.*",
        "~/.claude/*",
        "~/a*b",
        "~/*/bin",
        "~/v*/bin",
        "~/a\0b",
    ] {
        assert!(HomePath::new(path).is_err(), "{path:?} was admitted");
    }
}

#[test]
fn a_home_path_covers_itself_and_what_lies_inside_it() {
    let claude = HomePath::new("~/.claude").unwrap();
    let credentials = HomePath::new("~/.claude/.credentials.json").unwrap();
    let sibling = HomePath::new("~/.claude.json").unwrap();

    assert!(claude.covers(&claude));
    assert!(claude.covers(&credentials));
    assert!(!credentials.covers(&claude));
    assert!(!claude.covers(&sibling));
}

#[test]
fn a_wildcard_covers_each_name_with_its_prefix_and_what_lies_inside_it() {
    let backups = HomePath::new("~/.claude.json.*").unwrap();

    assert!(backups.covers(&HomePath::new("~/.claude.json.backup").unwrap()));
    assert!(backups.covers(&HomePath::new("~/.claude.json.1/x").unwrap()));
    assert!(!backups.covers(&HomePath::new("~/.claude.json").unwrap()));
}

#[test]
fn needs_refuse_their_first_malformed_value() {
    let refused = Needs::new(&["api.example.com", "*"], &[], &[]).unwrap_err();
    assert_eq!((refused.kind, refused.value.as_str()), ("host", "*"));

    let refused = Needs::new(&[], &["~/ok", "/etc"], &[]).unwrap_err();
    assert_eq!(
        (refused.kind, refused.value.as_str()),
        ("home path", "/etc")
    );

    let refused = Needs::new(&[], &[], &["~/.."]).unwrap_err();
    assert_eq!(
        (refused.kind, refused.value.as_str()),
        ("home path", "~/..")
    );
}

fn needs() -> Needs {
    Needs::new(
        &["api.example.com", "updates.example.com"],
        &["~/.local/share/example"],
        &["~/.example"],
    )
    .unwrap()
}

#[test]
fn a_credential_file_may_lie_in_a_folder_the_session_writes() {
    let credentials = Credentials::new(
        &["~/.example/auth.json"],
        &[(
            "EXAMPLE_API_KEY",
            CredentialHeader::Bearer,
            &["api.example.com"],
        )],
        &needs(),
    )
    .unwrap();

    assert_eq!(credentials.files[0].as_str(), "~/.example/auth.json");
    assert_eq!(credentials.bindings[0].variable, "EXAMPLE_API_KEY");
    assert_eq!(credentials.bindings[0].hosts[0].as_str(), "api.example.com");
}

#[test]
fn a_credential_file_a_read_path_would_expose_is_refused() {
    let refused = Credentials::new(&["~/.local/share/example/token"], &[], &needs()).unwrap_err();

    assert_eq!(refused.kind, "credential file");
    assert_eq!(refused.value, "~/.local/share/example/token");
}

#[test]
fn a_read_path_inside_a_credential_folder_is_refused() {
    let needs = Needs::new(&[], &["~/.config/github-copilot/hosts.json"], &[]).unwrap();

    let refused = Credentials::new(&["~/.config/github-copilot"], &[], &needs).unwrap_err();

    assert_eq!(refused.kind, "credential file");
    assert_eq!(refused.value, "~/.config/github-copilot");
}

#[test]
fn a_read_path_a_wildcard_credential_matches_is_refused() {
    let needs = Needs::new(&[], &["~/.token-cache"], &[]).unwrap();

    let refused = Credentials::new(&["~/.token*"], &[], &needs).unwrap_err();

    assert_eq!(refused.kind, "credential file");
}

#[test]
fn a_credential_host_the_program_does_not_declare_is_refused() {
    let refused = Credentials::new(
        &[],
        &[(
            "EXAMPLE_API_KEY",
            CredentialHeader::Bearer,
            &["api.example.com", "elsewhere.example"],
        )],
        &needs(),
    )
    .unwrap_err();

    assert_eq!(refused.kind, "credential host");
    assert_eq!(refused.value, "elsewhere.example");
}

#[test]
fn a_credential_variable_needs_a_valid_name_and_a_host() {
    for (variable, hosts) in [
        ("", &["api.example.com"][..]),
        ("1KEY", &["api.example.com"][..]),
        ("A-KEY", &["api.example.com"][..]),
        ("EXAMPLE_API_KEY", &[][..]),
    ] {
        let refused = Credentials::new(
            &[],
            &[(variable, CredentialHeader::Bearer, hosts)],
            &needs(),
        )
        .unwrap_err();
        assert_eq!(
            refused.kind, "credential variable",
            "{variable:?} {hosts:?}"
        );
    }
}

/// #968: the proxy replaces only an extension header, never one that frames or routes the
/// request.
#[test]
fn a_credential_header_is_a_lower_case_extension_header() {
    for header in [
        "host",
        "content-length",
        "x-",
        "X-Api-Key",
        "x-api key",
        "authorization",
    ] {
        let refused = Credentials::new(
            &[],
            &[(
                "EXAMPLE_API_KEY",
                CredentialHeader::Named(header),
                &["api.example.com"],
            )],
            &needs(),
        )
        .unwrap_err();
        assert_eq!(refused.kind, "credential header", "{header:?}");
    }
}

#[test]
fn an_api_key_header_is_declared_by_its_name() {
    let credentials = Credentials::new(
        &[],
        &[(
            "EXAMPLE_API_KEY",
            CredentialHeader::Named("x-api-key"),
            &["api.example.com"],
        )],
        &needs(),
    )
    .unwrap();

    assert_eq!(
        credentials.bindings[0].header,
        CredentialHeader::Named("x-api-key")
    );
}

#[test]
fn a_provider_that_declares_nothing_gets_the_floor() {
    let provider = cockpit_provider_api::fake::FakeProvider::new(
        cockpit_provider_api::ProviderName::new("fake").unwrap(),
        std::path::PathBuf::from("/nonexistent/agent.log"),
    );

    assert_eq!(provider.needs().unwrap(), Needs::none());
    assert_eq!(provider.credentials().unwrap(), Credentials::none());
}
