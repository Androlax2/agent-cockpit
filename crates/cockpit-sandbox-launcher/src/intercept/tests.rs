use std::cell::Cell;
use std::io::Cursor;

use super::*;

/// One side of a relay: what it sends, all at once, and what it received.
struct Canned {
    input: Cursor<Vec<u8>>,
    output: Vec<u8>,
}

impl Canned {
    fn new(input: &str) -> Canned {
        Canned {
            input: Cursor::new(input.as_bytes().to_vec()),
            output: Vec::new(),
        }
    }

    fn received(&self) -> String {
        String::from_utf8_lossy(&self.output).into_owned()
    }
}

impl Read for Canned {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.input.read(buffer)
    }
}

impl Write for Canned {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.output.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

const SECRET: &str = "github_pat_secret";

fn bearer() -> Credential {
    Credential::new(
        "the project's GitHub API token".to_owned(),
        CredentialScheme::Bearer,
        SECRET.to_owned(),
    )
    .unwrap()
}

fn git_reads() -> Credential {
    Credential::new(
        "the project's GitHub API token".to_owned(),
        CredentialScheme::GitReads {
            username: "x-access-token".to_owned(),
        },
        SECRET.to_owned(),
    )
    .unwrap()
}

struct Relayed {
    ended: Ended,
    /// What the upstream received.
    sent: String,
    /// What the client received.
    answered: String,
    credentials_added: u64,
}

fn relay_through(host: &str, credential: &Credential, requests: &str, responses: &str) -> Relayed {
    let mut client = Wire::new(Canned::new(requests));
    let mut upstream = Wire::new(Canned::new(responses));
    let added = Cell::new(0);
    let tally = |event: Event| {
        if let Event::CredentialAdded = event {
            added.set(added.get() + 1);
        }
    };
    let ended = relay(&mut client, &mut upstream, host, credential, &tally);
    Relayed {
        ended,
        sent: upstream.get_mut().received(),
        answered: client.get_mut().received(),
        credentials_added: added.get(),
    }
}

const OK_EMPTY: &str = "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
/// [`OK_EMPTY`] as the client gets it: every tunnel carries one exchange.
const OK_EMPTY_CLOSING: &str = "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";

fn basic() -> String {
    format!(
        "Authorization: Basic {}\r\n",
        base64(format!("x-access-token:{SECRET}").as_bytes())
    )
}

#[test]
fn bearer_replaces_whatever_authorization_the_client_sent() {
    let relayed = relay_through(
        "api.github.com",
        &bearer(),
        "GET /user HTTP/1.1\r\nHost: api.github.com\r\n\
         Authorization: token torlek-placeholder\r\n\r\n",
        "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}",
    );

    assert_eq!(relayed.ended, Ended::Closed);
    assert_eq!(
        relayed.sent,
        format!(
            "GET /user HTTP/1.1\r\nHost: api.github.com\r\nAuthorization: Bearer {SECRET}\r\n\
             Connection: close\r\n\r\n"
        )
    );
    assert_eq!(
        relayed.answered,
        "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}"
    );
    assert_eq!(relayed.credentials_added, 1);
}

/// A server may close a kept-alive connection while it sits idle, and a client cannot
/// retry a request that met the close: each tunnel carries one exchange, both heads saying so.
#[test]
fn a_tunnel_carries_one_exchange_and_both_sides_are_told_it_closes() {
    let relayed = relay_through(
        "api.github.com",
        &bearer(),
        "GET /a HTTP/1.1\r\nHost: api.github.com\r\nConnection: keep-alive\r\n\r\n\
         GET /b HTTP/1.1\r\nHost: api.github.com\r\n\r\n",
        "HTTP/1.1 200 OK\r\nContent-Length: 1\r\nKeep-Alive: timeout=5\r\n\r\nA\
         HTTP/1.1 200 OK\r\nContent-Length: 1\r\n\r\nB",
    );

    assert_eq!(relayed.ended, Ended::Closed);
    assert_eq!(
        relayed.sent,
        format!(
            "GET /a HTTP/1.1\r\nHost: api.github.com\r\nAuthorization: Bearer {SECRET}\r\n\
             Connection: close\r\n\r\n"
        )
    );
    assert_eq!(
        relayed.answered,
        "HTTP/1.1 200 OK\r\nContent-Length: 1\r\nConnection: close\r\n\r\nA"
    );
    assert_eq!(relayed.credentials_added, 1);
}

/// #857: a fetch's discovery gets the credential; the query decides, not the path alone.
#[test]
fn a_git_fetch_discovery_gets_basic_credentials() {
    let relayed = relay_through(
        "github.com",
        &git_reads(),
        "GET /owner/repo.git/info/refs?service=git-upload-pack HTTP/1.1\r\n\
         Host: github.com\r\n\r\n",
        OK_EMPTY,
    );

    assert_eq!(relayed.ended, Ended::Closed);
    assert!(relayed.sent.contains(&basic()), "{}", relayed.sent);
    assert_eq!(relayed.answered, OK_EMPTY_CLOSING);
}

#[test]
fn a_git_fetch_upload_pack_gets_basic_credentials_and_its_body() {
    let relayed = relay_through(
        "github.com",
        &git_reads(),
        "POST /owner/repo.git/git-upload-pack HTTP/1.1\r\nHost: github.com\r\n\
         Authorization: Basic Zm9vOmJhcg==\r\nContent-Length: 4\r\n\r\nwant",
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\npack\r\n0\r\n\r\n",
    );

    assert_eq!(relayed.ended, Ended::Closed);
    assert!(relayed.sent.contains(&basic()), "{}", relayed.sent);
    assert!(!relayed.sent.contains("Zm9vOmJhcg=="), "{}", relayed.sent);
    assert!(relayed.sent.ends_with("\r\n\r\nwant"), "{}", relayed.sent);
    assert!(
        relayed.answered.ends_with("4\r\npack\r\n0\r\n\r\n"),
        "{}",
        relayed.answered
    );
}

#[test]
fn a_push_discovery_is_refused_naming_torlek_push() {
    let relayed = relay_through(
        "github.com",
        &git_reads(),
        "GET /owner/repo.git/info/refs?service=git-receive-pack HTTP/1.1\r\n\
         Host: github.com\r\n\r\n",
        OK_EMPTY,
    );

    let Ended::Refused { reason, .. } = relayed.ended else {
        panic!("{:?}", relayed.ended);
    };
    assert!(reason.contains("`torlek push`"), "{reason}");
    assert_eq!(relayed.sent, "");
}

#[test]
fn a_push_upload_is_refused_however_its_path_is_spelled() {
    for target in [
        "/owner/repo.git/git-receive-pack",
        "/owner/repo.git/git-receive%2Dpack",
        "/owner/repo.git/GIT-RECEIVE-PACK/",
    ] {
        let relayed = relay_through(
            "github.com",
            &git_reads(),
            &format!("POST {target} HTTP/1.1\r\nHost: github.com\r\nContent-Length: 4\r\n\r\npack"),
            OK_EMPTY,
        );

        assert!(
            matches!(relayed.ended, Ended::Refused { .. }),
            "{target}: {:?}",
            relayed.ended
        );
        assert_eq!(relayed.sent, "", "{target}");
    }
}

/// Anything else on the Git host passes as the client sent it, with no credential.
#[test]
fn another_request_on_the_git_host_passes_without_the_credential() {
    let request = "GET /owner/repo/releases HTTP/1.1\r\nHost: github.com\r\n\
                   Authorization: token own\r\n\r\n";
    let relayed = relay_through("github.com", &git_reads(), request, OK_EMPTY);

    assert_eq!(relayed.ended, Ended::Closed);
    assert_eq!(
        relayed.sent,
        request.replace("\r\n\r\n", "\r\nConnection: close\r\n\r\n")
    );
    assert_eq!(relayed.credentials_added, 0);
}

/// The credential reaches no other service behind the same front end.
#[test]
fn a_request_naming_another_host_is_refused_before_it_goes_upstream() {
    for request in [
        "GET /user HTTP/1.1\r\nHost: uploads.github.com\r\n\r\n",
        "GET https://uploads.github.com/user HTTP/1.1\r\nHost: api.github.com\r\n\r\n",
        "GET /user HTTP/1.1\r\n\r\n",
        "GET /user HTTP/1.1\r\nHost: api.github.com\r\nHost: api.github.com\r\n\r\n",
    ] {
        let relayed = relay_through("api.github.com", &bearer(), request, OK_EMPTY);

        assert!(
            matches!(relayed.ended, Ended::Refused { .. }),
            "{request:?}: {:?}",
            relayed.ended
        );
        assert_eq!(relayed.sent, "");
    }
}

#[test]
fn the_host_may_carry_the_default_port_and_any_case() {
    let relayed = relay_through(
        "api.github.com",
        &bearer(),
        "GET https://API.github.com:443/user HTTP/1.1\r\nHost: Api.GitHub.com:443\r\n\r\n",
        OK_EMPTY,
    );

    assert_eq!(relayed.ended, Ended::Closed);
    assert_eq!(relayed.credentials_added, 1);
}

#[test]
fn a_protocol_upgrade_is_refused() {
    let relayed = relay_through(
        "api.github.com",
        &bearer(),
        "GET /socket HTTP/1.1\r\nHost: api.github.com\r\nConnection: Upgrade\r\n\
         Upgrade: websocket\r\n\r\n",
        OK_EMPTY,
    );

    assert!(matches!(relayed.ended, Ended::Refused { .. }));
    assert_eq!(relayed.sent, "");
}

/// Request smuggling: a body whose length two fields state differently is never guessed.
#[test]
fn an_ambiguous_request_body_is_a_bad_request() {
    for framing in [
        "Content-Length: 4\r\nTransfer-Encoding: chunked\r\n",
        "Content-Length: 4\r\nContent-Length: 5\r\n",
        "Content-Length: 4, 5\r\n",
        "Content-Length: +4\r\n",
        "Transfer-Encoding: chunked, gzip\r\n",
    ] {
        let relayed = relay_through(
            "api.github.com",
            &bearer(),
            &format!("POST /x HTTP/1.1\r\nHost: api.github.com\r\n{framing}\r\n0\r\n\r\n"),
            OK_EMPTY,
        );

        assert!(
            matches!(relayed.ended, Ended::BadRequest(_)),
            "{framing:?}: {:?}",
            relayed.ended
        );
        assert_eq!(relayed.sent, "");
    }
}

#[test]
fn a_chunked_request_body_passes_with_its_trailers() {
    let body = "5;ext=1\r\nhello\r\n0\r\nX-Trailer: 1\r\n\r\n";
    let relayed = relay_through(
        "api.github.com",
        &bearer(),
        &format!(
            "POST /x HTTP/1.1\r\nHost: api.github.com\r\nTransfer-Encoding: chunked\r\n\r\n{body}"
        ),
        OK_EMPTY,
    );

    assert_eq!(relayed.ended, Ended::Closed);
    assert!(relayed.sent.ends_with(body), "{}", relayed.sent);
}

/// The proxy answers `100 Continue` itself, so the client sends its body at once.
#[test]
fn an_expectation_of_100_continue_is_answered_by_the_proxy() {
    let relayed = relay_through(
        "api.github.com",
        &bearer(),
        "POST /x HTTP/1.1\r\nHost: api.github.com\r\nExpect: 100-continue\r\n\
         Content-Length: 2\r\n\r\nhi",
        OK_EMPTY,
    );

    assert_eq!(relayed.ended, Ended::Closed);
    assert!(!relayed.sent.contains("Expect"), "{}", relayed.sent);
    assert_eq!(
        relayed.answered,
        format!("HTTP/1.1 100 Continue\r\n\r\n{OK_EMPTY_CLOSING}")
    );
}

#[test]
fn an_interim_response_passes_before_the_final_one() {
    let interim = "HTTP/1.1 103 Early Hints\r\nLink: </a>\r\n\r\n";
    let relayed = relay_through(
        "api.github.com",
        &bearer(),
        "GET /x HTTP/1.1\r\nHost: api.github.com\r\n\r\n",
        &format!("{interim}{OK_EMPTY}"),
    );

    assert_eq!(relayed.ended, Ended::Closed);
    assert_eq!(relayed.answered, format!("{interim}{OK_EMPTY_CLOSING}"));
}

#[test]
fn a_response_without_a_length_runs_until_the_upstream_closes() {
    let relayed = relay_through(
        "api.github.com",
        &bearer(),
        "GET /x HTTP/1.1\r\nHost: api.github.com\r\n\r\n",
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\ndata: 1\n\n",
    );

    assert_eq!(relayed.ended, Ended::Closed);
    assert_eq!(
        relayed.answered,
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n\
         data: 1\n\n"
    );
    assert_eq!(relayed.credentials_added, 1);
}

#[test]
fn a_head_response_has_no_body_whatever_its_length_says() {
    let relayed = relay_through(
        "api.github.com",
        &bearer(),
        "HEAD /x HTTP/1.1\r\nHost: api.github.com\r\n\r\n",
        "HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n",
    );

    assert_eq!(relayed.ended, Ended::Closed);
    assert_eq!(
        relayed.answered,
        "HTTP/1.1 200 OK\r\nContent-Length: 10\r\nConnection: close\r\n\r\n"
    );
}

#[test]
fn an_ambiguous_response_body_breaks_the_exchange_before_its_head_reaches_the_client() {
    let relayed = relay_through(
        "api.github.com",
        &bearer(),
        "GET /x HTTP/1.1\r\nHost: api.github.com\r\n\r\n",
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Length: 5\r\n\r\n0\r\n\r\n",
    );

    assert!(matches!(relayed.ended, Ended::Broken(_)));
    assert_eq!(relayed.answered, "");
}

#[test]
fn an_upstream_that_closes_before_answering_breaks_the_exchange() {
    let relayed = relay_through(
        "api.github.com",
        &bearer(),
        "GET /x HTTP/1.1\r\nHost: api.github.com\r\n\r\n",
        "",
    );

    assert!(matches!(relayed.ended, Ended::Broken(_)));
}

#[test]
fn a_value_with_a_control_character_is_refused() {
    let refused = Credential::new(
        "token".to_owned(),
        CredentialScheme::Bearer,
        "a\r\nX-Injected: 1".to_owned(),
    );

    assert!(refused.is_err());
}

#[test]
fn base64_matches_known_encodings_with_each_padding() {
    for (plain, encoded) in [
        ("", ""),
        ("f", "Zg=="),
        ("ab", "YWI="),
        ("foo", "Zm9v"),
        ("foob", "Zm9vYg=="),
        ("fooba", "Zm9vYmE="),
        ("foobar", "Zm9vYmFy"),
    ] {
        assert_eq!(base64(plain.as_bytes()), encoded, "{plain}");
    }
}
