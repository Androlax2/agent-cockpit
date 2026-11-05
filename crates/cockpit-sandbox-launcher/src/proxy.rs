//! The session proxy (#831, #546 D1 stage A, D2): serves the listener stage 2 created inside
//! the sandbox, from the launcher on the host's network. One `CONNECT` per connection,
//! decided on the allowlist snapshot by `cockpit_sandbox`'s decision core; names resolved
//! here, every address checked, and on port 443 the TLS server name matched against the
//! `CONNECT` host. No interception: after the check the bytes pass unchanged.
//!
//! Answers tell policy from the network (#718): `403` with `Proxy-Status` error
//! `http_request_denied` or `destination_ip_prohibited` is a refusal; `502` (`dns_error`,
//! `destination_unavailable`) and `504` (`connection_timeout`) are an upstream the launcher
//! could not reach. The counters keep them apart the same way.

use std::io::{Read, Write};
use std::net::{IpAddr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cockpit_sandbox::{Allowlist, EgressRefusal, EgressReport, Subject, destination};

use crate::client_hello::{self, ServerName};

/// How long a client has to send its `CONNECT` request.
const HEAD_TIMEOUT: Duration = Duration::from_secs(30);
/// The longest request head read.
const MAX_HEAD_BYTES: usize = 16 * 1024;
/// How long a connection to an upstream address may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a client has to send its TLS ClientHello after `200`.
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
/// Connections served at once; past it, `503` at once.
const MAX_CONNECTIONS: usize = 1024;
/// How often the report file is rewritten while counters change.
const REPORT_INTERVAL: Duration = Duration::from_secs(1);
/// How long the proxy waits before accepting again after a failed accept, such as when
/// the launcher is out of file descriptors.
const ACCEPT_RETRY: Duration = Duration::from_millis(100);

/// Where the proxy resolves names and connects: the host's network in a launch, a fake in
/// tests.
pub trait Upstream: Send + Sync + 'static {
    fn resolve(&self, host: &str, port: u16) -> std::io::Result<Vec<IpAddr>>;
    fn connect(&self, address: SocketAddr, timeout: Duration) -> std::io::Result<TcpStream>;
}

/// The host's resolver and network.
pub struct Network;

impl Upstream for Network {
    fn resolve(&self, host: &str, port: u16) -> std::io::Result<Vec<IpAddr>> {
        Ok((host, port)
            .to_socket_addrs()?
            .map(|address| address.ip())
            .collect())
    }

    fn connect(&self, address: SocketAddr, timeout: Duration) -> std::io::Result<TcpStream> {
        TcpStream::connect_timeout(&address, timeout)
    }
}

pub struct Proxy<U> {
    allowlist: Allowlist,
    subject: Subject,
    upstream: U,
    report: Mutex<EgressReport>,
    /// Bumped at each change, so the writer rewrites the file only when something counted.
    changes: AtomicUsize,
    open: AtomicUsize,
    /// Held while the report file is written.
    writing: Mutex<()>,
}

/// Why a connection ends without a tunnel: the status, the `Proxy-Status` error and the
/// reason the body gives.
struct Answer {
    status: &'static str,
    error: &'static str,
    reason: String,
    is_refusal: bool,
}

impl Answer {
    fn refused(error: &'static str, reason: String) -> Answer {
        Answer {
            status: "403 Forbidden",
            error,
            reason,
            is_refusal: true,
        }
    }

    fn policy(refusal: &EgressRefusal) -> Answer {
        let error = if refusal.is_about_an_address() {
            "destination_ip_prohibited"
        } else {
            "http_request_denied"
        };
        Answer::refused(error, refusal.to_string())
    }

    fn unreachable(status: &'static str, error: &'static str, reason: String) -> Answer {
        Answer {
            status,
            error,
            reason,
            is_refusal: false,
        }
    }
}

impl<U: Upstream> Proxy<U> {
    pub fn new(allowlist: Allowlist, subject: Subject, upstream: U) -> Arc<Proxy<U>> {
        Arc::new(Proxy {
            allowlist,
            report: Mutex::new(EgressReport::new(subject.clone())),
            subject,
            upstream,
            changes: AtomicUsize::new(0),
            open: AtomicUsize::new(0),
            writing: Mutex::new(()),
        })
    }

    /// Serves `listener` until the process ends: one thread per connection.
    pub fn serve(self: &Arc<Self>, listener: TcpListener) {
        for client in listener.incoming() {
            let Ok(client) = client else {
                std::thread::sleep(ACCEPT_RETRY);
                continue;
            };
            let proxy = Arc::clone(self);
            let spawned = std::thread::Builder::new()
                .name("egress".to_owned())
                .spawn(move || proxy.handle(client));
            // Out of threads: the client's connection closes, which it sees at once.
            drop(spawned);
        }
    }

    /// A copy of the counters.
    pub fn report(&self) -> EgressReport {
        self.report
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Keeps `path` holding the report: rewritten each second something changed, until the
    /// process ends.
    pub fn keep_report(self: &Arc<Self>, path: PathBuf) {
        let proxy = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("egress-report".to_owned())
            .spawn(move || {
                let mut written = usize::MAX;
                loop {
                    let changes = proxy.changes.load(Ordering::Relaxed);
                    if changes != written && proxy.write_report(&path).is_ok() {
                        written = changes;
                    }
                    std::thread::sleep(REPORT_INTERVAL);
                }
            });
        drop(spawned);
    }

    /// Writes the report to `path` through a file beside it, so a reader never sees half.
    pub fn write_report(&self, path: &std::path::Path) -> std::io::Result<()> {
        // The report thread and the final write at exit share the staged file.
        let _writing = self
            .writing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let text = serde_json::to_vec(&self.report()).map_err(std::io::Error::other)?;
        let staged = path.with_extension("json.staged");
        std::fs::write(&staged, text)?;
        std::fs::rename(&staged, path)
    }

    fn count(&self, destination: &str, update: impl FnOnce(&mut cockpit_sandbox::EgressCounters)) {
        let mut report = self
            .report
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        update(report.counters(destination));
        self.changes.fetch_add(1, Ordering::Relaxed);
    }

    fn handle(&self, mut client: TcpStream) {
        if self.open.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
            self.open.fetch_sub(1, Ordering::SeqCst);
            let answer = Answer::unreachable(
                "503 Service Unavailable",
                "proxy_internal_error",
                format!("the session proxy serves {MAX_CONNECTIONS} connections at once"),
            );
            let _ = self.answer(&mut client, "", &answer);
            return;
        }
        self.tunnel(client);
        self.open.fetch_sub(1, Ordering::SeqCst);
    }

    fn tunnel(&self, mut client: TcpStream) {
        let (head, rest) = match read_head(&mut client) {
            Ok(read) => read,
            Err(answer) => {
                let _ = self.answer(&mut client, "", &answer);
                return;
            }
        };
        let (host, port) = match parse_connect(&head) {
            Ok(target) => target,
            Err(answer) => {
                let _ = self.answer(&mut client, "", &answer);
                return;
            }
        };
        // One spelling per name for the decision, the resolver and the counters' key.
        let host = cockpit_sandbox::normalized_name(&host).unwrap_or(host);
        let destination = destination(&host, port);
        let opened = match self.open_upstream(&host, port) {
            Ok(opened) => opened,
            Err(answer) => {
                self.count(&destination, |counters| {
                    if answer.is_refusal {
                        counters.refused += 1;
                    } else {
                        counters.unreachable += 1;
                    }
                    counters.last_reason = Some(answer.reason.clone());
                });
                let _ = self.answer(&mut client, &destination, &answer);
                return;
            }
        };
        let (mut upstream, source, expects_server_name) = opened;
        if client
            .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
            .is_err()
        {
            return;
        }
        let mut first_bytes = rest;
        if expects_server_name {
            match self.check_server_name(&mut client, &host, &mut first_bytes) {
                Ok(()) => {}
                Err(reason) => {
                    self.count(&destination, |counters| {
                        counters.refused += 1;
                        counters.last_reason = Some(reason);
                    });
                    let _ = client.shutdown(Shutdown::Both);
                    return;
                }
            }
        }
        self.count(&destination, |counters| {
            counters.allowed += 1;
            counters.source = Some(source);
        });
        if upstream.write_all(&first_bytes).is_err() {
            return;
        }
        let _ = client.set_read_timeout(None);
        self.splice(client, upstream, &destination, first_bytes.len() as u64);
    }

    /// Decides, resolves, checks and connects: the upstream stream, the allowing entry's
    /// source and whether the server name is checked; or the answer to give.
    fn open_upstream(&self, host: &str, port: u16) -> Result<(TcpStream, String, bool), Answer> {
        let allowed = self
            .allowlist
            .decide(host, port)
            .map_err(|refusal| Answer::policy(&refusal))?;
        let addresses = if allowed.is_name {
            let addresses = self.upstream.resolve(host, port).map_err(|error| {
                Answer::unreachable(
                    "502 Bad Gateway",
                    "dns_error",
                    format!("{} does not resolve: {error}", destination(host, port)),
                )
            })?;
            self.allowlist
                .check_addresses(host, port, &addresses)
                .map_err(|refusal| Answer::policy(&refusal))?;
            addresses
        } else {
            vec![host.parse().expect("an address literal decided as one")]
        };
        if addresses.is_empty() {
            return Err(Answer::unreachable(
                "502 Bad Gateway",
                "dns_error",
                format!("{} resolves to no address", destination(host, port)),
            ));
        }
        let mut last_error = None;
        for address in addresses {
            match self
                .upstream
                .connect(SocketAddr::new(address, port), CONNECT_TIMEOUT)
            {
                Ok(stream) => {
                    return Ok((
                        stream,
                        allowed.entry.source.to_string(),
                        allowed.expects_server_name,
                    ));
                }
                Err(error) => last_error = Some(error),
            }
        }
        let error = last_error.expect("at least one address was tried");
        let reason = format!("{} is unreachable: {error}", destination(host, port));
        Err(if error.kind() == std::io::ErrorKind::TimedOut {
            Answer::unreachable("504 Gateway Timeout", "connection_timeout", reason)
        } else {
            Answer::unreachable("502 Bad Gateway", "destination_unavailable", reason)
        })
    }

    /// Reads the client's TLS ClientHello into `first_bytes` and checks it names `host`.
    fn check_server_name(
        &self,
        client: &mut TcpStream,
        host: &str,
        first_bytes: &mut Vec<u8>,
    ) -> Result<(), String> {
        let _ = client.set_read_timeout(Some(HELLO_TIMEOUT));
        let expected = cockpit_sandbox::normalized_name(host).unwrap_or_default();
        loop {
            match client_hello::server_name(first_bytes) {
                ServerName::Named(name) if name == expected => return Ok(()),
                ServerName::Named(name) => {
                    return Err(format!(
                        "the TLS server name {name} is not the CONNECT host {expected}"
                    ));
                }
                ServerName::Missing => {
                    return Err(format!(
                        "the TLS ClientHello for {expected} names no server"
                    ));
                }
                ServerName::NotTls => {
                    return Err(format!(
                        "the tunnel to {expected}:443 does not start with TLS"
                    ));
                }
                ServerName::Incomplete => {}
            }
            if first_bytes.len() > client_hello::MAX_HELLO_BYTES {
                return Err("the TLS ClientHello is too long".to_owned());
            }
            let mut chunk = [0_u8; 4096];
            match client.read(&mut chunk) {
                Ok(0) => return Err("the client closed before its TLS ClientHello".to_owned()),
                Ok(read) => first_bytes.extend_from_slice(&chunk[..read]),
                Err(error) => {
                    return Err(format!("no TLS ClientHello arrived: {error}"));
                }
            }
        }
    }

    /// Copies both ways until the upstream ends, or each side does, counting the bytes.
    fn splice(&self, client: TcpStream, upstream: TcpStream, destination: &str, already_out: u64) {
        let (Ok(client_reader), Ok(upstream_reader)) = (client.try_clone(), upstream.try_clone())
        else {
            return;
        };
        std::thread::scope(|scope| {
            scope.spawn(|| {
                self.copy(client_reader, &upstream, destination, Direction::Out);
            });
            self.count(destination, |counters| counters.bytes_out += already_out);
            self.copy(upstream_reader, &client, destination, Direction::In);
        });
    }

    fn copy(&self, mut from: TcpStream, to: &TcpStream, destination: &str, direction: Direction) {
        let mut writer = to;
        let mut buffer = vec![0_u8; 16 * 1024];
        loop {
            let read = match from.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => read,
            };
            if writer.write_all(&buffer[..read]).is_err() {
                break;
            }
            // Counted as they pass, so the report written at exit misses no tunnel's bytes.
            self.add_bytes(destination, direction, read as u64);
        }
        match direction {
            // A client may half-close and still read the answer.
            Direction::Out => {
                let _ = to.shutdown(Shutdown::Write);
            }
            // Once the upstream ended, a client left open would hold its thread and its
            // connection slot for good.
            Direction::In => {
                let _ = to.shutdown(Shutdown::Both);
                let _ = from.shutdown(Shutdown::Both);
            }
        }
    }

    fn add_bytes(&self, destination: &str, direction: Direction, bytes: u64) {
        if bytes == 0 {
            return;
        }
        self.count(destination, |counters| match direction {
            Direction::Out => counters.bytes_out += bytes,
            Direction::In => counters.bytes_in += bytes,
        });
    }

    fn answer(
        &self,
        client: &mut TcpStream,
        destination: &str,
        answer: &Answer,
    ) -> std::io::Result<()> {
        let subject = match &self.subject {
            Subject::Session { session } => format!("session {session}"),
        };
        let target = if destination.is_empty() {
            String::new()
        } else {
            format!(" {destination}")
        };
        let verdict = if answer.is_refusal {
            "refused"
        } else {
            "could not open"
        };
        let body = format!(
            "torlek-sandbox {verdict}{target} for {subject}: {}\n",
            answer.reason
        );
        // The reason can quote what the client sent: only printable ASCII goes in a header.
        let details: String = answer
            .reason
            .chars()
            .map(|character| match character {
                '"' | '\\' => '\'',
                ' '..='~' => character,
                _ => '?',
            })
            .collect();
        let response = format!(
            "HTTP/1.1 {}\r\nProxy-Status: torlek-sandbox; error={}; details=\"{details}\"\r\n\
             Content-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\n\
             Connection: close\r\n\r\n{body}",
            answer.status,
            answer.error,
            body.len()
        );
        client.write_all(response.as_bytes())?;
        client.flush()
    }
}

#[derive(Clone, Copy)]
enum Direction {
    Out,
    In,
}

/// The request head, and the bytes the client sent after it.
fn read_head(client: &mut TcpStream) -> Result<(String, Vec<u8>), Answer> {
    let _ = client.set_read_timeout(Some(HEAD_TIMEOUT));
    let mut received = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        if let Some(end) = received.windows(4).position(|window| window == b"\r\n\r\n") {
            let rest = received.split_off(end + 4);
            let head = String::from_utf8(received).map_err(|_| bad_request("is not UTF-8"))?;
            return Ok((head, rest));
        }
        if received.len() > MAX_HEAD_BYTES {
            return Err(bad_request("is longer than 16 KiB"));
        }
        match client.read(&mut chunk) {
            Ok(0) => return Err(bad_request("ended before its blank line")),
            Ok(read) => received.extend_from_slice(&chunk[..read]),
            Err(_) => return Err(bad_request("did not arrive in time")),
        }
    }
}

fn bad_request(what: &str) -> Answer {
    Answer {
        status: "400 Bad Request",
        error: "http_request_error",
        reason: format!("the request head {what}"),
        is_refusal: true,
    }
}

/// The host and port of a `CONNECT host:port HTTP/1.x` request line. Any other method is
/// refused: plain HTTP is not carried, so no request is forwarded unread.
fn parse_connect(head: &str) -> Result<(String, u16), Answer> {
    let line = head.lines().next().unwrap_or_default();
    let mut words = line.split(' ');
    let (Some(method), Some(target), Some(version), None) =
        (words.next(), words.next(), words.next(), words.next())
    else {
        return Err(bad_request("has no request line"));
    };
    if !version.starts_with("HTTP/1.") {
        return Err(bad_request("is not HTTP/1"));
    }
    if method != "CONNECT" {
        return Err(Answer::refused(
            "http_request_denied",
            format!(
                "{method} {target}: only CONNECT is carried; plain HTTP never leaves the \
                 sandbox, use HTTPS"
            ),
        ));
    }
    let (host, port) = match target.strip_prefix('[') {
        Some(bracketed) => bracketed
            .split_once("]:")
            .ok_or_else(|| bad_request("names no port after an IPv6 address"))?,
        None => target
            .rsplit_once(':')
            .ok_or_else(|| bad_request("names no port"))?,
    };
    let port = port
        .parse::<u16>()
        .ok()
        .filter(|port| *port != 0)
        .ok_or_else(|| bad_request("names no port from 1 to 65535"))?;
    if host.is_empty() {
        return Err(bad_request("names no host"));
    }
    Ok((host.to_owned(), port))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::Instant;

    use cockpit_sandbox::{AllowEntry, AllowSource, Target};

    use super::*;
    use crate::client_hello::tests::client_hello;

    /// Names resolve as the map says; every connection goes to `server` on the test host's
    /// loopback, whatever address and port the proxy asked for.
    struct Fake {
        names: HashMap<&'static str, IpAddr>,
        server: Option<SocketAddr>,
    }

    impl Upstream for Fake {
        fn resolve(&self, host: &str, _port: u16) -> std::io::Result<Vec<IpAddr>> {
            self.names
                .get(host)
                .map(|address| vec![*address])
                .ok_or_else(|| std::io::Error::other("no such name"))
        }

        fn connect(&self, _address: SocketAddr, timeout: Duration) -> std::io::Result<TcpStream> {
            let server = self
                .server
                .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::ConnectionRefused))?;
            TcpStream::connect_timeout(&server, timeout)
        }
    }

    const PUBLIC: [u8; 4] = [93, 184, 215, 14];

    fn name(name: &str, port: u16) -> AllowEntry {
        AllowEntry {
            target: Target::Name(name.to_owned()),
            port,
            source: AllowSource::Adapter {
                provider: "claude".to_owned(),
            },
        }
    }

    /// A server that echoes what it reads, prefixed with `greeting` sent first.
    fn echo_server(greeting: &'static [u8]) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                std::thread::spawn(move || {
                    let _ = stream.write_all(greeting);
                    let mut reader = stream.try_clone().unwrap();
                    let _ = std::io::copy(&mut reader, &mut stream);
                });
            }
        });
        address
    }

    struct Running {
        address: SocketAddr,
        proxy: Arc<Proxy<Fake>>,
    }

    fn start(entries: Vec<AllowEntry>, upstream: Fake) -> Running {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let proxy = Proxy::new(
            Allowlist {
                program: "claude".to_owned(),
                entries,
            },
            Subject::Session {
                session: "s1".to_owned(),
            },
            upstream,
        );
        let serving = Arc::clone(&proxy);
        std::thread::spawn(move || serving.serve(listener));
        Running { address, proxy }
    }

    fn fake(names: &[(&'static str, [u8; 4])], server: Option<SocketAddr>) -> Fake {
        Fake {
            names: names
                .iter()
                .map(|(name, address)| (*name, IpAddr::from(*address)))
                .collect(),
            server,
        }
    }

    /// Sends `CONNECT target`, then `then`; returns everything the proxy sent back until it
    /// closed or two seconds passed.
    fn connect(proxy: SocketAddr, target: &str, then: &[u8]) -> String {
        let mut stream = TcpStream::connect(proxy).unwrap();
        stream
            .write_all(format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n").as_bytes())
            .unwrap();
        stream.write_all(then).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut received = Vec::new();
        let _ = stream.read_to_end(&mut received);
        String::from_utf8_lossy(&received).into_owned()
    }

    #[test]
    fn an_unknown_host_is_refused_at_once_naming_it_and_the_session() {
        let running = start(vec![name("api.anthropic.com", 443)], fake(&[], None));
        let started = Instant::now();

        let answer = connect(running.address, "evil.example.com:443", b"");

        assert!(
            started.elapsed() < Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
        assert!(answer.starts_with("HTTP/1.1 403 Forbidden\r\n"), "{answer}");
        assert!(answer.contains("error=http_request_denied"), "{answer}");
        assert!(
            answer.contains(
                "torlek-sandbox refused evil.example.com:443 for session s1: \
                 evil.example.com:443 is not allowed"
            ),
            "{answer}"
        );
    }

    /// #861: the owner learns the program must declare its hosts.
    #[test]
    fn a_program_declaring_no_hosts_is_named_in_the_refusal() {
        let running = start(Vec::new(), fake(&[], None));

        let answer = connect(running.address, "api.anthropic.com:443", b"");

        assert!(answer.contains("`claude` declares no hosts"), "{answer}");
    }

    #[test]
    fn an_ip_literal_is_refused_unless_granted_by_address() {
        let running = start(vec![name("api.anthropic.com", 443)], fake(&[], None));

        for target in [
            "93.184.215.14:443",
            "[2606:2800:21f:cb07:6820:80da:af6b:8b2c]:443",
        ] {
            let answer = connect(running.address, target, b"");

            assert!(answer.starts_with("HTTP/1.1 403 "), "{target}: {answer}");
            assert!(
                answer.contains("error=destination_ip_prohibited"),
                "{answer}"
            );
        }
    }

    /// An allowed name that resolves to a private, loopback or metadata address is refused:
    /// DNS rebinding reaches nothing inside the owner's network.
    #[test]
    fn an_allowed_name_resolving_outside_the_public_internet_is_refused() {
        for address in [
            [127, 0, 0, 1],
            [10, 1, 2, 3],
            [169, 254, 169, 254],
            [192, 168, 1, 1],
        ] {
            let running = start(
                vec![name("api.anthropic.com", 443)],
                fake(&[("api.anthropic.com", address)], Some(echo_server(b""))),
            );

            let answer = connect(running.address, "api.anthropic.com:443", b"");

            assert!(answer.starts_with("HTTP/1.1 403 "), "{address:?}: {answer}");
            assert!(
                answer.contains("error=destination_ip_prohibited"),
                "{answer}"
            );
        }
    }

    #[test]
    fn a_tunnel_to_443_whose_server_name_matches_passes_its_bytes_unchanged() {
        let running = start(
            vec![name("api.anthropic.com", 443)],
            fake(&[("api.anthropic.com", PUBLIC)], Some(echo_server(b""))),
        );
        let hello = client_hello(Some("api.anthropic.com"), 16384);

        let answer = connect(running.address, "api.anthropic.com:443", &hello);

        let established = "HTTP/1.1 200 Connection established\r\n\r\n";
        assert!(answer.starts_with(established), "{answer}");
        assert_eq!(
            answer.as_bytes()[established.len()..],
            String::from_utf8_lossy(&hello).as_bytes()[..]
        );
    }

    #[test]
    fn a_tunnel_to_443_with_another_or_no_server_name_is_closed_unanswered() {
        for hello in [
            client_hello(Some("evil.example.com"), 16384),
            client_hello(None, 16384),
        ] {
            let running = start(
                vec![name("api.anthropic.com", 443)],
                fake(&[("api.anthropic.com", PUBLIC)], Some(echo_server(b""))),
            );

            let answer = connect(running.address, "api.anthropic.com:443", &hello);

            assert_eq!(answer, "HTTP/1.1 200 Connection established\r\n\r\n");
            let report = running.proxy.report();
            let counters = &report.destinations["api.anthropic.com:443"];
            assert_eq!((counters.allowed, counters.refused), (0, 1));
        }
    }

    /// A granted port other than 443 is a plain tunnel: a server that speaks first, such as
    /// SSH, is heard without the client sending anything.
    #[test]
    fn a_granted_port_other_than_443_tunnels_without_a_peek() {
        let running = start(
            vec![name("git.example.com", 22)],
            fake(
                &[("git.example.com", PUBLIC)],
                Some(echo_server(b"SSH-2.0-test\r\n")),
            ),
        );

        let answer = connect(running.address, "git.example.com:22", b"");

        assert_eq!(
            answer,
            "HTTP/1.1 200 Connection established\r\n\r\nSSH-2.0-test\r\n"
        );
    }

    /// #718: a host that does not answer is the network's failure, not a refusal.
    #[test]
    fn an_unreachable_upstream_is_a_502_counted_apart_from_refusals() {
        let running = start(
            vec![
                name("api.anthropic.com", 443),
                name("down.example.com", 443),
            ],
            fake(&[("api.anthropic.com", PUBLIC)], None),
        );

        let refused_connection = connect(running.address, "api.anthropic.com:443", b"");
        let unresolved = connect(running.address, "down.example.com:443", b"");

        assert!(
            refused_connection.starts_with("HTTP/1.1 502 "),
            "{refused_connection}"
        );
        assert!(
            refused_connection.contains("error=destination_unavailable"),
            "{refused_connection}"
        );
        assert!(unresolved.contains("error=dns_error"), "{unresolved}");
        let report = running.proxy.report();
        let counters = &report.destinations["api.anthropic.com:443"];
        assert_eq!((counters.refused, counters.unreachable), (0, 1));
    }

    #[test]
    fn plain_http_is_refused_naming_why() {
        let running = start(vec![name("api.anthropic.com", 443)], fake(&[], None));
        let mut stream = TcpStream::connect(running.address).unwrap();
        stream
            .write_all(b"GET http://api.anthropic.com/ HTTP/1.1\r\nHost: api.anthropic.com\r\n\r\n")
            .unwrap();
        let mut answer = String::new();
        let _ = stream.read_to_string(&mut answer);

        assert!(answer.starts_with("HTTP/1.1 403 "), "{answer}");
        assert!(answer.contains("only CONNECT is carried"), "{answer}");
    }

    /// However a client spells a name, it is decided, resolved and counted as one.
    #[test]
    fn spellings_of_one_name_are_counted_under_it() {
        let running = start(
            vec![name("git.example.com", 22)],
            fake(&[("git.example.com", PUBLIC)], Some(echo_server(b"hi"))),
        );

        let spelled = connect(running.address, "Git.Example.COM.:22", b"");
        connect(running.address, "UNKNOWN.example.com:22", b"");
        connect(running.address, "unknown.example.com.:22", b"");

        assert!(spelled.starts_with("HTTP/1.1 200 "), "{spelled}");
        let report = running.proxy.report();
        assert_eq!(report.destinations["git.example.com:22"].allowed, 1);
        assert_eq!(report.destinations["unknown.example.com:22"].refused, 2);
        assert_eq!(report.destinations.len(), 2);
    }

    /// An upstream that closed ends the tunnel, even when the client keeps its side open.
    #[test]
    fn a_tunnel_ends_when_its_upstream_closes() {
        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        let server_address = server.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = server.accept().unwrap();
            stream.write_all(b"bye").unwrap();
        });
        let running = start(
            vec![name("git.example.com", 22)],
            fake(&[("git.example.com", PUBLIC)], Some(server_address)),
        );
        let mut client = TcpStream::connect(running.address).unwrap();
        client
            .write_all(b"CONNECT git.example.com:22 HTTP/1.1\r\n\r\n")
            .unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();

        let mut received = Vec::new();
        let ended = client.read_to_end(&mut received);

        assert!(ended.is_ok(), "{ended:?}");
        assert_eq!(
            String::from_utf8_lossy(&received),
            "HTTP/1.1 200 Connection established\r\n\r\nbye"
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while running.proxy.open.load(Ordering::SeqCst) != 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(running.proxy.open.load(Ordering::SeqCst), 0);
    }

    /// What a client sent never starts a header line of the answer.
    #[test]
    fn a_reason_quoting_control_characters_keeps_the_answer_head_intact() {
        let running = start(Vec::new(), fake(&[], None));
        let mut stream = TcpStream::connect(running.address).unwrap();
        stream
            .write_all(b"GET\rX-Injected:1 http://a/ HTTP/1.1\r\n\r\n")
            .unwrap();
        let mut answer = Vec::new();
        let _ = stream.read_to_end(&mut answer);

        let answer = String::from_utf8_lossy(&answer).into_owned();
        let head = &answer[..answer.find("\r\n\r\n").expect("an answer head")];
        assert!(answer.starts_with("HTTP/1.1 4"), "{answer:?}");
        assert!(
            head.split("\r\n").all(|line| !line.contains('\r')),
            "{head:?}"
        );
    }

    #[test]
    fn a_wildcard_entry_allows_names_under_its_domain_only() {
        let running = start(
            vec![name("*.example.com", 22)],
            fake(
                &[("a.example.com", PUBLIC), ("example.com", PUBLIC)],
                Some(echo_server(b"hi")),
            ),
        );

        let under = connect(running.address, "a.example.com:22", b"");
        let apex = connect(running.address, "example.com:22", b"");

        assert!(under.starts_with("HTTP/1.1 200 "), "{under}");
        assert!(apex.starts_with("HTTP/1.1 403 "), "{apex}");
    }
}
