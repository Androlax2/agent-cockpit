//! The values the contract validates where they are made.

use cockpit_forge_api::{BranchName, ChangeId, CommitId, ForgeName, IssueId, ListLimit, Token};

#[test]
fn a_forge_name_is_lower_case_letters_digits_and_dashes() {
    for name in ["fake", "forge-2", "a"] {
        assert!(ForgeName::new(name).is_ok(), "{name}");
    }
    for name in ["", "Forge", "2forge", "-forge", "for ge", &"a".repeat(33)] {
        assert!(ForgeName::new(name).is_err(), "{name}");
    }
}

#[test]
fn issue_and_change_request_ids_are_words_safe_in_a_url_path() {
    for id in ["1", "PROJ-12", &"9".repeat(64)] {
        assert!(IssueId::new(id).is_ok(), "{id}");
        assert!(ChangeId::new(id).is_ok(), "{id}");
    }
    for id in [
        "",
        "1 2",
        "1\n",
        "é",
        ".",
        "..",
        "../x",
        "a/b",
        "1?x",
        "1#x",
        "%2e",
        &"9".repeat(65),
    ] {
        assert!(IssueId::new(id).is_err(), "{id:?}");
        assert!(ChangeId::new(id).is_err(), "{id:?}");
    }
}

#[test]
fn a_branch_name_is_one_git_accepts() {
    for name in ["main", "greenfield/153-forge-api", "release/v1.2", "a-b_c"] {
        assert!(BranchName::new(name).is_ok(), "{name}");
    }
    let refused = [
        "",
        "-main",
        "/main",
        "main/",
        "main.",
        "main.lock",
        "a..b",
        "a//b",
        "a@{1}",
        "@",
        "a b",
        "a~1",
        "a^",
        "a:b",
        "a?",
        "a*",
        "a[",
        "a\\b",
        ".hidden",
        "a/.hidden",
        "a\u{7f}",
        "topic.lock/fix",
        "HEAD",
    ];
    for name in refused {
        assert!(BranchName::new(name).is_err(), "{name:?}");
    }
}

#[test]
fn a_commit_id_is_a_full_lower_case_object_id() {
    assert!(CommitId::new("a".repeat(40)).is_ok());
    assert!(CommitId::new("0123456789abcdef".repeat(4)).is_ok());
    for id in [
        "a".repeat(39),
        "a".repeat(41),
        "A".repeat(40),
        "g".repeat(40),
        "a".repeat(63),
    ] {
        assert!(CommitId::new(id.clone()).is_err(), "{id}");
    }
}

#[test]
fn a_list_limit_is_one_to_a_hundred() {
    assert_eq!(ListLimit::new(1).unwrap().get(), 1);
    assert_eq!(ListLimit::new(100).unwrap().get(), 100);
    assert!(ListLimit::new(0).is_err());
    assert!(ListLimit::new(101).is_err());
}

#[test]
fn a_token_never_prints_its_value() {
    let token = Token::new("secret-value-123").unwrap();

    assert_eq!(format!("{token:?}"), "Token(<hidden>)");
    assert_eq!(token.expose(), "secret-value-123");
}

#[test]
fn a_refused_token_is_not_repeated_in_the_error() {
    for value in ["", "secret value", "secret\n", "sécret"] {
        let error = Token::new(value).unwrap_err();

        assert_eq!(error.value, "<hidden>", "{value:?}");
    }
}
