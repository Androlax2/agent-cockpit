//! What reaches `gh`: the token in `GH_TOKEN` alone, a folder of its own, no variable of the
//! owner's, and bodies on standard input.

mod common;

use cockpit_forge_api::{Forge, IssueId, NewIssue};
use common::{FakeGh, TOKEN, with_access};

/// The variables the adapter may pass `gh`; anything else came from the daemon's environment.
const ALLOWED: [&str; 25] = [
    "PATH",
    "HTTPS_PROXY",
    "https_proxy",
    "HTTP_PROXY",
    "http_proxy",
    "NO_PROXY",
    "no_proxy",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "GH_CONFIG_DIR",
    "HOME",
    "XDG_CONFIG_HOME",
    "XDG_STATE_HOME",
    "XDG_CACHE_HOME",
    "XDG_DATA_HOME",
    "GH_HOST",
    "GH_PROMPT_DISABLED",
    "GH_NO_UPDATE_NOTIFIER",
    "GH_NO_EXTENSION_UPDATE_NOTIFIER",
    "NO_COLOR",
    "GH_PAGER",
    "GH_TOKEN",
    // The shell the fake `gh` runs in sets these itself.
    "PWD",
    "SHLVL",
    "_",
];

fn commented() -> FakeGh {
    let mut gh = FakeGh::new();
    gh.answer_with(
        &["issue", "view", "12", "--repo=owner/project", "--json=url"],
        "issue-view-url.json",
    )
    .answer_with(
        &[
            "issue",
            "comment",
            "12",
            "--repo=owner/project",
            "--body-file=-",
        ],
        "issue-comment.stdout",
    );
    with_access(|access| {
        gh.adapter()
            .comment_on_issue(access, &IssueId::new("12").unwrap(), "The body.")
            .unwrap()
    });
    gh
}

/// The comment's call, after the read that makes sure the number is an issue's.
fn comment_call(gh: &FakeGh) -> common::Call {
    let mut calls = gh.calls();
    assert_eq!(calls.len(), 2, "{calls:#?}");
    calls.pop().unwrap()
}

#[test]
fn the_token_reaches_gh_as_gh_token() {
    let call = comment_call(&commented());

    assert_eq!(
        call.environment.get("GH_TOKEN").map(String::as_str),
        Some(TOKEN)
    );
}

#[test]
fn no_variable_of_the_daemons_environment_reaches_gh_beyond_the_allowed_ones() {
    let call = comment_call(&commented());

    let passed: Vec<&String> = call
        .environment
        .keys()
        .filter(|name| !ALLOWED.contains(&name.as_str()))
        .collect();
    assert!(passed.is_empty(), "{passed:?}");
}

#[test]
fn gh_keeps_its_configuration_in_the_adapters_folder_never_the_owners() {
    let gh = commented();
    let call = comment_call(&gh);
    let config_dir = gh.config_dir().display().to_string();

    for name in ["GH_CONFIG_DIR", "HOME"] {
        assert_eq!(call.environment.get(name), Some(&config_dir), "{name}");
    }
    assert!(
        call.environment["XDG_CONFIG_HOME"].starts_with(&config_dir),
        "{:?}",
        call.environment["XDG_CONFIG_HOME"]
    );
}

#[test]
fn gh_runs_in_its_folder_so_no_checkouts_remote_is_inferred() {
    let gh = commented();

    assert_eq!(comment_call(&gh).folder, gh.config_dir());
}

#[test]
fn a_body_reaches_gh_on_standard_input_never_in_its_arguments() {
    let call = comment_call(&commented());

    assert_eq!(call.stdin, "The body.");
    assert!(
        !call
            .arguments
            .iter()
            .any(|argument| argument.contains("The body."))
    );
}

#[test]
fn the_token_is_never_an_argument() {
    let call = comment_call(&commented());

    assert!(
        !call
            .arguments
            .iter()
            .any(|argument| argument.contains(TOKEN))
    );
}

#[test]
fn gh_version_runs_without_a_token() {
    let gh = FakeGh::new();
    let _adapter = gh.adapter();

    let calls = gh.every_call();
    assert_eq!(calls.len(), 1, "{calls:#?}");
    assert_eq!(calls[0].arguments, ["--version"]);
    assert!(!calls[0].environment.contains_key("GH_TOKEN"));
}

#[test]
fn a_title_starting_with_a_dash_stays_the_titles_value() {
    let mut gh = FakeGh::new();
    gh.answer_with(
        &[
            "issue",
            "create",
            "--repo=owner/project",
            "--title=--web",
            "--body-file=-",
        ],
        "issue-create.stdout",
    );

    let created = with_access(|access| {
        gh.adapter().create_issue(
            access,
            &NewIssue {
                title: "--web".to_owned(),
                body: String::new(),
                labels: Vec::new(),
            },
        )
    });

    assert_eq!(created.unwrap().id.as_str(), "13");
}
