//! Which adapter serves a project's origin, and the named reason when none does (#857,
//! decision 4).

use cockpit_forge_api::fake::{FakeForge, GIT_HOST};
use cockpit_forge_api::{ForgeHost, Repository, RepositoryPath, Unresolved, parse_origin, resolve};

fn repository(host: &str, path: &str) -> Repository {
    Repository {
        host: ForgeHost::new(host).unwrap(),
        path: RepositoryPath::new(path).unwrap(),
    }
}

#[test]
fn an_https_origin_names_its_host_and_path() {
    let cases = [
        (
            "https://git.example.com/owner/name",
            "git.example.com",
            "owner/name",
        ),
        (
            "https://git.example.com/owner/name.git",
            "git.example.com",
            "owner/name",
        ),
        (
            "https://git.example.com/owner/name.git/",
            "git.example.com",
            "owner/name",
        ),
        (
            "https://Git.Example.com/owner/name",
            "git.example.com",
            "owner/name",
        ),
        (
            "HTTPS://git.example.com/owner/name",
            "git.example.com",
            "owner/name",
        ),
        (
            "https://git.example.com:8443/group/sub/name",
            "git.example.com:8443",
            "group/sub/name",
        ),
        (
            "https://git.example.com/owner/.config",
            "git.example.com",
            "owner/.config",
        ),
    ];

    for (origin, host, path) in cases {
        assert_eq!(parse_origin(origin), Ok(repository(host, path)), "{origin}");
    }
}

#[test]
fn an_ssh_origin_is_refused_as_not_supported_yet() {
    let origins = [
        "git@git.example.com:owner/name.git",
        "git.example.com:owner/name",
        "ssh://git@git.example.com/owner/name.git",
        "git+ssh://git.example.com/owner/name",
        "ssh+git://git.example.com/owner/name",
    ];

    for origin in origins {
        assert_eq!(parse_origin(origin), Err(Unresolved::Ssh), "{origin}");
    }
    assert_eq!(
        Unresolved::Ssh.to_string(),
        "SSH remotes are not supported yet"
    );
}

#[test]
fn an_origin_on_another_scheme_is_refused_naming_it() {
    for (origin, scheme) in [
        ("http://git.example.com/owner/name", "http"),
        ("git://git.example.com/owner/name", "git"),
        ("file:///srv/name.git", "file"),
    ] {
        assert_eq!(
            parse_origin(origin),
            Err(Unresolved::Scheme {
                scheme: scheme.to_owned()
            }),
            "{origin}"
        );
    }
}

#[test]
fn a_local_path_is_not_a_url() {
    for origin in ["/srv/name.git", "../name", "name"] {
        assert_eq!(parse_origin(origin), Err(Unresolved::NotAUrl), "{origin}");
    }
}

#[test]
fn an_origin_carrying_a_user_or_password_is_refused_without_repeating_it() {
    let origin = "https://user:secret-token@git.example.com/owner/name";

    let refusal = parse_origin(origin).unwrap_err();

    assert_eq!(refusal, Unresolved::UserInfo);
    assert!(!refusal.to_string().contains("secret-token"));
    assert_eq!(
        parse_origin("https://oauth2@git.example.com/owner/name"),
        Err(Unresolved::UserInfo)
    );
}

#[test]
fn a_malformed_https_origin_is_refused() {
    let origins = [
        "https://git.example.com",
        "https://git.example.com/",
        "https://git.example.com/owner//name",
        "https://git.example.com/owner/../name",
        "https://git.example.com/owner/name?ref=x",
        "https://git.example.com/owner/name#x",
        "https://git.example.com/owner/na%6De",
        "https://git.example.com/owner/na me",
        "https://127.0.0.1/owner/name",
        "https://localhost/owner/name",
        "https://git.example.com/owner/name.git.git",
    ];

    for origin in origins {
        assert!(
            matches!(parse_origin(origin), Err(Unresolved::Malformed { .. })),
            "{origin}: {:?}",
            parse_origin(origin)
        );
    }
}

#[test]
fn the_adapter_declaring_the_origin_s_git_host_serves_it() {
    let fake = FakeForge::new();

    let (forge, repository) =
        resolve(&[&fake], &format!("https://{GIT_HOST}/owner/project.git")).unwrap();

    assert_eq!(forge.name().as_str(), "fake");
    assert_eq!(&repository, fake.repository());
}

#[test]
fn an_origin_on_an_undeclared_host_has_no_adapter() {
    let fake = FakeForge::new();

    let refusal = resolve(&[&fake], "https://other.example.com/owner/project")
        .map(drop)
        .unwrap_err();

    assert_eq!(
        refusal.to_string(),
        "no adapter for this forge: other.example.com"
    );
}

/// The API host is declared, but an origin there is not a Git remote the adapter serves.
#[test]
fn an_origin_on_an_api_host_has_no_adapter() {
    let fake = FakeForge::new();

    let refusal = resolve(&[&fake], "https://api.forge.example.com/owner/project").map(drop);

    assert!(
        matches!(refusal, Err(Unresolved::NoAdapter { .. })),
        "{refusal:?}"
    );
}

#[test]
fn two_adapters_declaring_one_git_host_are_refused() {
    let first = FakeForge::new();
    let second = FakeForge::new();

    let refusal = resolve(
        &[&first, &second],
        &format!("https://{GIT_HOST}/owner/project"),
    )
    .map(drop);

    assert!(
        matches!(refusal, Err(Unresolved::Ambiguous { .. })),
        "{refusal:?}"
    );
}

#[test]
fn a_repository_its_adapter_refuses_is_refused_with_the_adapter_s_reason() {
    let fake = FakeForge::new();

    let refusal = resolve(&[&fake], &format!("https://{GIT_HOST}/group/sub/project"))
        .map(drop)
        .unwrap_err();

    assert_eq!(
        refusal.to_string(),
        "the fake adapter refuses this origin: a repository path is `owner/name`"
    );
}

#[test]
fn no_refusal_repeats_the_origin() {
    let origin = "https://user:hunter2@git.example.com/owner/name";
    let fake = FakeForge::new();

    let refusal = resolve(&[&fake], origin).map(drop).unwrap_err();

    assert!(!format!("{refusal} {refusal:?}").contains("hunter2"));
}

#[test]
fn a_malformed_refusal_repeats_no_part_of_the_origin() {
    for origin in [
        "https://git.example.com:secret99/owner/name",
        "https://git.example.com/secret99/../name",
    ] {
        let refusal = parse_origin(origin).unwrap_err();

        assert!(
            matches!(refusal, Unresolved::Malformed { .. }),
            "{refusal:?}"
        );
        assert!(
            !format!("{refusal} {refusal:?}").contains("secret99"),
            "{refusal}"
        );
    }
}
