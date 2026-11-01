//! The profile Cockpit requests: what it carries on each platform, and each rule a platform
//! or request cannot enforce, refused with the rule and the reason rather than weakened.

mod common;

use std::path::PathBuf;

use cockpit_sandbox::{DesktopSession, Platform, Profile, ProfileRequest, Refusal};
use common::{profile_request, wsl2};

fn refusal(request: ProfileRequest, platform: &Platform) -> Refusal {
    Profile::build(request, platform).expect_err("the profile is refused")
}

#[test]
fn the_profile_denies_reading_everything_but_the_checkout_and_cockpits_folders() {
    let profile = Profile::build(profile_request("/home/owner/app"), &Platform::Linux).unwrap();

    let rules = profile.rules();

    for expected in [
        "platform linux",
        "provider claude",
        "read-write /home/owner/app",
        "read-only /run/user/1000/cockpit/agent",
        "read everything else: denied",
        "deny /home/owner/.local/state/cockpit/cockpit.db",
        "deny read ~/.ssh/id_*",
        "deny write .git/hooks",
        "network: through greyproxy only",
        "secret COCKPIT_LAUNCH_TOKEN passed through",
        "desktop session withheld",
    ] {
        assert!(
            rules.iter().any(|rule| rule == expected),
            "{expected} missing from {rules:#?}"
        );
    }
}

#[test]
fn an_unsupported_platform_is_refused_by_name() {
    let refused = refusal(
        profile_request("/home/owner/app"),
        &Platform::Unsupported("freebsd".to_owned()),
    );

    assert_eq!(refused.rule, "sandbox");
    assert!(refused.reason.contains("freebsd"), "{refused}");
}

#[test]
fn a_checkout_inside_a_denied_folder_is_refused_instead_of_handed_over_unreadable() {
    let mut request = profile_request("/home/owner/.local/state/cockpit/app");
    request.denied = vec![PathBuf::from("/home/owner/.local/state/cockpit")];

    let refused = refusal(request, &Platform::Linux);

    assert_eq!(
        refused.rule,
        "read-write /home/owner/.local/state/cockpit/app"
    );
    assert!(
        refused.reason.contains("which the profile denies"),
        "{refused}"
    );
}

#[test]
fn a_relative_path_is_refused() {
    let refused = refusal(profile_request("app"), &Platform::Linux);

    assert_eq!(refused.rule, "read-write app");
}

#[test]
fn a_checkout_on_a_windows_drive_is_refused_under_wsl2() {
    let refused = refusal(profile_request("/mnt/c/Users/owner/app"), &wsl2());

    assert_eq!(refused.rule, "read-write /mnt/c/Users/owner/app");
    assert!(refused.reason.contains("Windows drive"), "{refused}");
}

#[test]
fn wsl2_denies_the_windows_drives_and_interop_and_drops_the_interop_variable() {
    let mut request = profile_request("/home/owner/app");
    request
        .environment
        .push(("WSL_INTEROP".into(), "/run/WSL/1_interop".into()));

    let profile = Profile::build(request, &wsl2()).unwrap();

    for denied in ["/mnt/c", "/mnt/d", "/run/WSL"] {
        assert!(
            profile.denied.contains(&PathBuf::from(denied)),
            "{denied} missing from {:?}",
            profile.denied
        );
    }
    assert!(
        profile
            .environment
            .iter()
            .all(|(name, _)| name != "WSL_INTEROP")
    );
}

#[test]
fn a_withheld_desktop_session_refuses_an_environment_holding_a_desktop_variable() {
    let mut request = profile_request("/home/owner/app");
    request.environment.push(("DISPLAY".into(), ":0".into()));

    let refused = refusal(request, &Platform::Linux);

    assert_eq!(refused.rule, "desktop withheld");
    assert!(refused.reason.contains("DISPLAY"), "{refused}");
}

#[test]
fn a_shared_desktop_session_keeps_its_variables() {
    let mut request = profile_request("/home/owner/app");
    request.desktop = DesktopSession::Shared;
    request.environment.push(("DISPLAY".into(), ":0".into()));

    let profile = Profile::build(request, &Platform::Linux).unwrap();

    assert!(
        profile
            .environment
            .iter()
            .any(|(name, _)| name == "DISPLAY")
    );
    assert!(
        profile
            .rules()
            .contains(&"desktop session shared".to_owned())
    );
}

#[test]
fn a_malformed_allowed_host_is_refused() {
    let mut request = profile_request("/home/owner/app");
    request.allow_hosts = vec!["example.com:http".to_owned()];

    let refused = refusal(request, &Platform::Linux);

    assert_eq!(refused.rule, "allow host example.com:http");
}

#[test]
fn a_provider_name_outside_lowercase_letters_digits_and_dashes_is_refused() {
    let mut request = profile_request("/home/owner/app");
    request.provider = "../claude".to_owned();

    let refused = refusal(request, &Platform::Linux);

    assert_eq!(refused.rule, "provider ../claude");
}
