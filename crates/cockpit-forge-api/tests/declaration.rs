//! The hosts an adapter declares and the token each takes (#857).

use cockpit_forge_api::{
    CredentialScheme, CredentialUse, Declaration, ForgeHost, HostUse, TokenKind,
};

fn host(name: &str) -> ForgeHost {
    ForgeHost::new(name).unwrap()
}

fn credential(
    token: TokenKind,
    name: &str,
    scheme: CredentialScheme,
    used_for: HostUse,
) -> CredentialUse {
    CredentialUse {
        token,
        host: host(name),
        scheme,
        used_for,
    }
}

fn git_read() -> CredentialUse {
    credential(
        TokenKind::Api,
        "git.example.com",
        CredentialScheme::Basic,
        HostUse::GitRead,
    )
}

fn git_push() -> CredentialUse {
    credential(
        TokenKind::Push,
        "git.example.com",
        CredentialScheme::Basic,
        HostUse::GitPush,
    )
}

fn api() -> CredentialUse {
    credential(
        TokenKind::Api,
        "api.example.com",
        CredentialScheme::Bearer,
        HostUse::Api,
    )
}

#[test]
fn a_forge_host_is_one_named_public_host() {
    for name in [
        "git.example.com",
        "example.com",
        "git.example.com:8443",
        "a-b.example.org",
    ] {
        assert!(ForgeHost::new(name).is_ok(), "{name}");
    }
    let refused = [
        "",
        "*.example.com",
        "*",
        "localhost",
        "git.localhost",
        "example",
        "127.0.0.1",
        "0x7f000001",
        "Git.example.com",
        "git.example.com:0",
        "git.example.com:65536",
        "git.example.com:0443",
        "-git.example.com",
        "git..example.com",
        "git.example.com/",
        "user@git.example.com",
    ];
    for name in refused {
        assert!(ForgeHost::new(name).is_err(), "{name:?}");
    }
}

#[test]
fn a_declaration_lists_its_hosts_and_the_git_ones() {
    let declaration = Declaration::new(vec![git_read(), git_push(), api()]).unwrap();

    assert_eq!(
        declaration.hosts().into_iter().cloned().collect::<Vec<_>>(),
        [host("api.example.com"), host("git.example.com")]
    );
    assert_eq!(
        declaration
            .git_hosts()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>(),
        [host("git.example.com")]
    );
}

#[test]
fn the_proxy_and_the_push_each_read_their_own_token_s_uses() {
    let declaration = Declaration::new(vec![git_read(), git_push(), api()]).unwrap();

    assert_eq!(
        declaration
            .uses_of(TokenKind::Api)
            .cloned()
            .collect::<Vec<_>>(),
        [git_read(), api()]
    );
    assert_eq!(
        declaration
            .uses_of(TokenKind::Push)
            .cloned()
            .collect::<Vec<_>>(),
        [git_push()]
    );
}

/// #857, decision 1: the token the proxy adds never writes contents, so the push token is
/// for pushes alone and a push takes nothing else.
#[test]
fn a_declaration_keeps_the_push_token_to_pushes_and_pushes_to_it() {
    let refused = [
        credential(
            TokenKind::Api,
            "git.example.com",
            CredentialScheme::Basic,
            HostUse::GitPush,
        ),
        credential(
            TokenKind::Push,
            "git.example.com",
            CredentialScheme::Basic,
            HostUse::GitRead,
        ),
        credential(
            TokenKind::Push,
            "api.example.com",
            CredentialScheme::Bearer,
            HostUse::Api,
        ),
    ];

    for wrong in refused {
        assert!(
            Declaration::new(vec![git_read(), wrong.clone()]).is_err(),
            "{wrong:?}"
        );
    }
}

#[test]
fn a_declaration_without_a_git_host_is_refused() {
    assert!(Declaration::new(vec![api()]).is_err());
    assert!(Declaration::new(Vec::new()).is_err());
}

#[test]
fn a_use_declared_twice_is_refused() {
    assert!(Declaration::new(vec![git_read(), git_read()]).is_err());
}
