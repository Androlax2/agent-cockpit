//! The session proxy with a bound credential (#832), end to end over TLS: a real handshake
//! with the per-session CA's certificate on one side, an upstream server with its own CA on
//! the other.

use std::path::Path;

use cockpit_sandbox::{
    CredentialHost, CredentialScheme, Interception, PLACEHOLDER, SpecCredential,
};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

use super::*;

const HOST: &str = "api.github.com";
const GIT_HOST: &str = "github.com";
const PROVIDER_HOST: &str = "api.anthropic.com";
const SECRET: &str = "github_pat_secret";
const TOKEN_NAME: &str = "the project's GitHub API token";

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// A CA of the upstream's own, as a public server has, and the PEM of its certificate.
fn upstream_ca() -> (rcgen::CertifiedIssuer<'static, rcgen::KeyPair>, String) {
    let mut params = rcgen::CertificateParams::default();
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    params.key_usages = vec![rcgen::KeyUsagePurpose::KeyCertSign];
    let issuer =
        rcgen::CertifiedIssuer::self_signed(params, rcgen::KeyPair::generate().unwrap()).unwrap();
    let pem = issuer.pem();
    (issuer, pem)
}

/// A TLS server for `host`, signed by `ca`, that answers each request with its own head as
/// the body, then closes.
fn upstream_server(ca: &rcgen::CertifiedIssuer<'static, rcgen::KeyPair>, host: &str) -> SocketAddr {
    let key = rcgen::KeyPair::generate().unwrap();
    let certificate = rcgen::CertificateParams::new(vec![host.to_owned()])
        .unwrap()
        .signed_by(&key, ca)
        .unwrap();
    let mut config = rustls::ServerConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![certificate.der().clone()],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
        )
        .unwrap();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let config = Arc::new(config);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let config = Arc::clone(&config);
            std::thread::spawn(move || {
                let connection = rustls::ServerConnection::new(config).unwrap();
                let mut tls = rustls::StreamOwned::new(connection, stream.unwrap());
                let mut head = Vec::new();
                let mut byte = [0_u8; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    match tls.read(&mut byte) {
                        Ok(1) => head.push(byte[0]),
                        _ => return,
                    }
                }
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    head.len()
                );
                let _ = tls.write_all(response.as_bytes());
                let _ = tls.write_all(&head);
                tls.conn.send_close_notify();
                let _ = tls.flush();
            });
        }
    });
    address
}

/// The launcher's authority for `HOST`, whose upstream roots are `roots_pem`, with its trust
/// files in `folder`.
fn authority(folder: &Path, roots_pem: &str) -> (Authority, Interception) {
    authority_binding(folder, roots_pem, HOST, CredentialScheme::Bearer)
}

/// The launcher's authority adding the credential on `host` by `scheme`.
fn authority_binding(
    folder: &Path,
    roots_pem: &str,
    host: &str,
    scheme: CredentialScheme,
) -> (Authority, Interception) {
    let system_bundle = folder.join("system-bundle.pem");
    std::fs::write(&system_bundle, roots_pem).unwrap();
    let interception = Interception {
        credentials: vec![SpecCredential {
            name: TOKEN_NAME.to_owned(),
            hosts: vec![CredentialHost {
                host: host.to_owned(),
                scheme,
            }],
        }],
        system_bundle,
        ca_file: folder.join("session-ca.pem"),
        bundle_file: folder.join("ca-bundle.pem"),
    };
    let authority = Authority::create("s1", &interception, vec![SECRET.to_owned()]).unwrap();
    (authority, interception)
}

fn running(authority: Authority, server: SocketAddr) -> Running {
    start_with(
        vec![
            name(HOST, 443),
            name(GIT_HOST, 443),
            name(PROVIDER_HOST, 443),
        ],
        fake(
            &[(HOST, PUBLIC), (GIT_HOST, PUBLIC), (PROVIDER_HOST, PUBLIC)],
            Some(server),
        ),
        Some(authority),
    )
}

/// `CONNECT HOST:443`, then TLS trusting `roots_pem` alone and one request; what the client
/// read back, or why its TLS failed.
fn request_through(proxy: SocketAddr, roots_pem: &str, request: &str) -> Result<String, String> {
    request_to(proxy, HOST, roots_pem, request)
}

/// [`request_through`] for `host`.
fn request_to(
    proxy: SocketAddr,
    host: &str,
    roots_pem: &str,
    request: &str,
) -> Result<String, String> {
    let mut stream = TcpStream::connect(proxy).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .write_all(format!("CONNECT {host}:443 HTTP/1.1\r\nHost: {host}:443\r\n\r\n").as_bytes())
        .unwrap();
    let mut established = Vec::new();
    let mut byte = [0_u8; 1];
    while !established.ends_with(b"\r\n\r\n") {
        assert_eq!(stream.read(&mut byte).unwrap(), 1);
        established.push(byte[0]);
    }
    assert!(established.starts_with(b"HTTP/1.1 200 "));
    let mut roots = rustls::RootCertStore::empty();
    for certificate in CertificateDer::pem_slice_iter(roots_pem.as_bytes()) {
        roots.add(certificate.unwrap()).unwrap();
    }
    let config = rustls::ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let connection = rustls::ClientConnection::new(
        Arc::new(config),
        rustls::pki_types::ServerName::try_from(host.to_owned()).unwrap(),
    )
    .unwrap();
    let mut tls = rustls::StreamOwned::new(connection, stream);
    tls.write_all(request.as_bytes())
        .map_err(|error| error.to_string())?;
    let mut received = Vec::new();
    match tls.read_to_end(&mut received) {
        Ok(_) => Ok(String::from_utf8_lossy(&received).into_owned()),
        Err(error) if !received.is_empty() => {
            // A server may close without `close_notify` after a complete answer.
            let _ = error;
            Ok(String::from_utf8_lossy(&received).into_owned())
        }
        Err(error) => Err(error.to_string()),
    }
}

/// Waits until the report's counters for `HOST:443` satisfy `ready`.
fn counters_when(
    running: &Running,
    ready: impl Fn(&cockpit_sandbox::EgressCounters) -> bool,
) -> cockpit_sandbox::EgressCounters {
    counters_of(running, HOST, ready)
}

/// [`counters_when`] for `host:443`.
fn counters_of(
    running: &Running,
    host: &str,
    ready: impl Fn(&cockpit_sandbox::EgressCounters) -> bool,
) -> cockpit_sandbox::EgressCounters {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let counters = running
            .proxy
            .report()
            .destinations
            .get(&format!("{host}:443"))
            .cloned()
            .unwrap_or_default();
        if ready(&counters) || Instant::now() > deadline {
            return counters;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

const REQUEST: &str = "GET /user HTTP/1.1\r\nHost: api.github.com\r\n\
                       Authorization: token torlek-placeholder\r\n\r\n";

#[test]
fn a_bound_host_gets_the_credential_and_the_client_never_holds_it() {
    let folder = tempfile::tempdir().unwrap();
    let (ca, ca_pem) = upstream_ca();
    let (authority, interception) = authority(folder.path(), &ca_pem);
    let running = running(authority, upstream_server(&ca, HOST));
    let session_ca = std::fs::read_to_string(&interception.ca_file).unwrap();

    let answer = request_through(running.address, &session_ca, REQUEST).unwrap();

    assert!(answer.starts_with("HTTP/1.1 200 OK\r\n"), "{answer}");
    assert!(
        answer.contains(&format!("Authorization: Bearer {SECRET}\r\n")),
        "{answer}"
    );
    assert!(!answer.contains("torlek-placeholder"), "{answer}");
    let counters = counters_when(&running, |counters| counters.credential_added == 1);
    assert_eq!((counters.allowed, counters.credential_added), (1, 1));
}

/// `SSL_CERT_FILE` and its kin replace a runtime's roots: the bundle keeps the host's.
#[test]
fn the_bundle_file_holds_the_hosts_roots_then_the_session_ca() {
    let folder = tempfile::tempdir().unwrap();
    let (ca, ca_pem) = upstream_ca();
    let (authority, interception) = authority(folder.path(), &ca_pem);
    let running = running(authority, upstream_server(&ca, HOST));
    let bundle = std::fs::read_to_string(&interception.bundle_file).unwrap();
    let session_ca = std::fs::read_to_string(&interception.ca_file).unwrap();

    let answer = request_through(running.address, &bundle, REQUEST).unwrap();

    assert!(answer.contains("Bearer"), "{answer}");
    assert!(bundle.starts_with(&ca_pem), "{bundle}");
    assert!(bundle.ends_with(&session_ca), "{bundle}");
}

/// A client that trusts only its own roots, as one that pins its certificates does, fails
/// its handshake, and the counters name the host.
#[test]
fn a_client_refusing_the_session_ca_is_named_with_the_host() {
    let folder = tempfile::tempdir().unwrap();
    let (ca, ca_pem) = upstream_ca();
    let (authority, _) = authority(folder.path(), &ca_pem);
    let running = running(authority, upstream_server(&ca, HOST));

    let failed = request_through(running.address, &ca_pem, REQUEST);

    assert!(failed.is_err(), "{failed:?}");
    let counters = counters_when(&running, |counters| counters.ca_refused == 1);
    assert_eq!(counters.ca_refused, 1);
    let reason = counters.last_reason.unwrap_or_default();
    assert!(reason.contains("for api.github.com"), "{reason}");
    assert!(reason.contains("pins its certificates"), "{reason}");
}

/// The proxy verifies the upstream against the host's roots; one they do not trust is the
/// network's failure (#718), told to the client in a `502`.
#[test]
fn an_upstream_the_host_roots_do_not_trust_is_a_502() {
    let folder = tempfile::tempdir().unwrap();
    let (ca, _) = upstream_ca();
    let (_, other_roots) = upstream_ca();
    let (authority, interception) = authority(folder.path(), &other_roots);
    let running = running(authority, upstream_server(&ca, HOST));
    let session_ca = std::fs::read_to_string(&interception.ca_file).unwrap();

    let answer = request_through(running.address, &session_ca, REQUEST).unwrap();

    assert!(answer.starts_with("HTTP/1.1 502 Bad Gateway: "), "{answer}");
    assert!(answer.contains("error=tls_certificate_error"), "{answer}");
    assert!(!answer.contains(SECRET), "{answer}");
    let counters = counters_when(&running, |counters| counters.unreachable == 1);
    assert_eq!((counters.unreachable, counters.credential_added), (1, 0));
}

/// A refusal inside the tunnel is answered there, with `Proxy-Status`, and counted.
#[test]
fn a_request_for_another_host_is_answered_403_inside_the_tunnel() {
    let folder = tempfile::tempdir().unwrap();
    let (ca, ca_pem) = upstream_ca();
    let (authority, interception) = authority(folder.path(), &ca_pem);
    let running = running(authority, upstream_server(&ca, HOST));
    let session_ca = std::fs::read_to_string(&interception.ca_file).unwrap();

    let answer = request_through(
        running.address,
        &session_ca,
        "GET /x HTTP/1.1\r\nHost: uploads.github.com\r\n\r\n",
    )
    .unwrap();

    assert!(answer.starts_with("HTTP/1.1 403 Forbidden: "), "{answer}");
    assert!(answer.contains("error=http_request_denied"), "{answer}");
    let counters = counters_when(&running, |counters| counters.refused == 1);
    assert_eq!((counters.refused, counters.credential_added), (1, 0));
}

/// Every host without a bound credential stays a plain tunnel: its bytes pass unchanged.
#[test]
fn a_host_without_a_credential_stays_a_plain_tunnel() {
    let folder = tempfile::tempdir().unwrap();
    let (_, ca_pem) = upstream_ca();
    let (authority, _) = authority(folder.path(), &ca_pem);
    let running = running(authority, echo_server(b""));
    let hello = client_hello(Some("api.anthropic.com"), 16384);

    let answer = connect(running.address, "api.anthropic.com:443", &hello);

    let established = "HTTP/1.1 200 Connection established\r\n\r\n";
    assert!(answer.starts_with(established), "{answer}");
    assert_eq!(
        answer.as_bytes()[established.len()..],
        String::from_utf8_lossy(&hello).as_bytes()[..]
    );
}

fn git_reads() -> CredentialScheme {
    CredentialScheme::GitReads {
        username: "x-access-token".to_owned(),
    }
}

/// #857: both halves of a push are answered by the proxy, naming `torlek push`; nothing
/// reaches GitHub and the token is added to neither.
#[test]
fn a_git_push_over_https_is_answered_403_naming_torlek_push() {
    for request in [
        "GET /owner/repo.git/info/refs?service=git-receive-pack HTTP/1.1\r\nHost: github.com\r\n\r\n",
        "POST /owner/repo.git/git-receive-pack HTTP/1.1\r\nHost: github.com\r\n\
         Content-Length: 4\r\n\r\npack",
    ] {
        let folder = tempfile::tempdir().unwrap();
        let (ca, ca_pem) = upstream_ca();
        let (authority, interception) =
            authority_binding(folder.path(), &ca_pem, GIT_HOST, git_reads());
        let running = running(authority, upstream_server(&ca, GIT_HOST));
        let session_ca = std::fs::read_to_string(&interception.ca_file).unwrap();

        let answer = request_to(running.address, GIT_HOST, &session_ca, request).unwrap();

        assert!(answer.starts_with("HTTP/1.1 403 Forbidden: "), "{answer}");
        assert!(answer.contains("`torlek push`"), "{answer}");
        assert!(!answer.contains(SECRET), "{answer}");
        let counters = counters_of(&running, GIT_HOST, |counters| counters.refused == 1);
        assert_eq!(
            (counters.refused, counters.credential_added),
            (1, 0),
            "{request}"
        );
    }
}

/// #857: a fetch's discovery on the same `info/refs` path gets the token as Basic.
#[test]
fn a_git_fetch_discovery_gets_the_token_as_basic_through_the_proxy() {
    let folder = tempfile::tempdir().unwrap();
    let (ca, ca_pem) = upstream_ca();
    let (authority, interception) =
        authority_binding(folder.path(), &ca_pem, GIT_HOST, git_reads());
    let running = running(authority, upstream_server(&ca, GIT_HOST));
    let session_ca = std::fs::read_to_string(&interception.ca_file).unwrap();

    let answer = request_to(
        running.address,
        GIT_HOST,
        &session_ca,
        "GET /owner/repo.git/info/refs?service=git-upload-pack HTTP/1.1\r\nHost: github.com\r\n\r\n",
    )
    .unwrap();

    assert!(answer.starts_with("HTTP/1.1 200 OK\r\n"), "{answer}");
    // base64 of `x-access-token:github_pat_secret`.
    assert!(
        answer.contains("Authorization: Basic eC1hY2Nlc3MtdG9rZW46Z2l0aHViX3BhdF9zZWNyZXQ=\r\n"),
        "{answer}"
    );
    let counters = counters_of(&running, GIT_HOST, |counters| {
        counters.credential_added == 1
    });
    assert_eq!(counters.credential_added, 1);
}

/// #858: a provider's request to its API host carries the real key in the header its
/// binding names, in place of the placeholder the agent holds.
#[test]
fn a_providers_request_to_its_host_carries_the_key_in_place_of_the_placeholder() {
    let folder = tempfile::tempdir().unwrap();
    let (ca, ca_pem) = upstream_ca();
    let scheme = CredentialScheme::Header {
        name: "x-api-key".to_owned(),
    };
    let (authority, interception) =
        authority_binding(folder.path(), &ca_pem, PROVIDER_HOST, scheme);
    let running = running(authority, upstream_server(&ca, PROVIDER_HOST));
    let session_ca = std::fs::read_to_string(&interception.ca_file).unwrap();

    let answer = request_to(
        running.address,
        PROVIDER_HOST,
        &session_ca,
        &format!(
            "POST /v1/messages HTTP/1.1\r\nHost: api.anthropic.com\r\n\
             x-api-key: {PLACEHOLDER}\r\nContent-Length: 0\r\n\r\n"
        ),
    )
    .unwrap();

    assert!(
        answer.contains(&format!("x-api-key: {SECRET}\r\n")),
        "{answer}"
    );
    assert!(!answer.contains(PLACEHOLDER), "{answer}");
}

/// #858: the same placeholder sent to a host with no bound credential passes as the agent
/// wrote it, through a plain tunnel, and carries no credential.
#[test]
fn the_placeholder_sent_to_another_host_carries_no_credential() {
    let folder = tempfile::tempdir().unwrap();
    let (ca, ca_pem) = upstream_ca();
    let scheme = CredentialScheme::Header {
        name: "x-api-key".to_owned(),
    };
    let (authority, _) = authority_binding(folder.path(), &ca_pem, PROVIDER_HOST, scheme);
    let running = running(authority, upstream_server(&ca, GIT_HOST));

    let answer = request_to(
        running.address,
        GIT_HOST,
        &ca_pem,
        &format!("GET / HTTP/1.1\r\nHost: github.com\r\nx-api-key: {PLACEHOLDER}\r\n\r\n"),
    )
    .unwrap();

    assert!(
        answer.contains(&format!("x-api-key: {PLACEHOLDER}\r\n")),
        "{answer}"
    );
    assert!(!answer.contains(SECRET), "{answer}");
    let counters = counters_of(&running, GIT_HOST, |counters| counters.allowed == 1);
    assert_eq!((counters.allowed, counters.credential_added), (1, 0));
}
