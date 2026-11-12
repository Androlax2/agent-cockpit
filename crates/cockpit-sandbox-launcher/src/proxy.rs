//! The session proxy (#831, #546 D1 stage A, D2): serves the listener stage 2 created inside
//! the sandbox, from the launcher on the host's network. One `CONNECT` per connection,
//! decided on the allowlist snapshot by `cockpit_sandbox`'s decision core; names resolved
//! here, every address checked, and on port 443 the TLS server name matched against the
//! `CONNECT` host. After the check the bytes pass unchanged, except on a host that carries a
//! bound credential (#832): there the proxy ends the client's TLS with the per-session CA's
//! certificate, opens its own to the host, and relays HTTP/1.1, adding the credential
//! (`intercept.rs`).
//!
//! The owner's decisions on network requests (#674) arrive in a file the daemon replaces
//! (`cockpit_sandbox::DECISIONS_FILE`): read before serving and again each time it changes.
//! A destination refused for want of an entry goes to a file the daemon reads
//! (`cockpit_sandbox::REQUESTS_FILE`), which asks the owner; the refusal itself is at once. A
//! revoked decision closes the tunnels it allowed. The same decisions and requests files
//! carry the declared services the launcher forwards (`forward.rs`, #880).
//!
//! Answers tell policy from the network (#718): `403` with `Proxy-Status` error
//! `http_request_denied` or `destination_ip_prohibited` is a refusal; `502` (`dns_error`,
//! `destination_unavailable`) and `504` (`connection_timeout`) are an upstream the launcher
//! could not reach. The counters keep them apart the same way. The status line carries the
//! reason after its reason phrase (#966): Python, pip and Go print a failed `CONNECT`'s status
//! line and `curl -v` shows it, while no client prints the body. Once the daemon filed the
//! request, the reason names its number and the owner's command.

use std::collections::{BTreeSet, HashMap};
use std::io::{Read, Write};
use std::net::{IpAddr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;

use cockpit_sandbox::{
    AllowEntry, Allowlist, AskedDestination, AskedService, EgressDecisions, EgressRefusal,
    EgressReport, EgressRequests, MAX_ASKED_DESTINATIONS, Subject, destination,
};

use crate::authority::{Authority, BoundHost};
use crate::client_hello::{self, ServerName};
use crate::intercept::{self, Direction, Ended, Event, Wire};

mod forward;
mod live;

pub use forward::{Listener, bind_sockets, check_engine_socket};

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
/// The longest reason the status line and `Proxy-Status` carry: a refusal of the longest DNS
/// name (253 bytes) still ends with the owner's command.
const MAX_HEADER_TEXT: usize = 512;
/// How often the decisions file is read for a change: an owner's decision applies to the
/// agent's next try within this.
const DECISIONS_INTERVAL: Duration = Duration::from_millis(200);
/// How long an intercepted connection's client may stay silent: between requests, or inside
/// one's body. Clients reopen a kept-alive connection the proxy closed.
const IDLE_TIMEOUT: Duration = Duration::from_secs(300);

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
    /// The spec's allowlist; the owner's decisions add to it, never take from it.
    allowlist: Allowlist,
    /// The spec's allowlist with the owner's decisions, and the decisions themselves.
    decided: RwLock<Arc<Decided>>,
    /// Where refused destinations are listed for the daemon; none in tests that ask nothing.
    requests_file: OnceLock<PathBuf>,
    /// The destinations listed there, held while the file is written.
    asked: Mutex<Asked>,
    /// The tunnels open now, by number, so a revoked decision closes those it allowed.
    tunnels: Mutex<HashMap<u64, OpenTunnel>>,
    /// The forwards' relays open now, numbered beside the tunnels (#880).
    relays: Mutex<HashMap<u64, forward::OpenRelay>>,
    /// Binds the listeners of forwards opened live (#390); unset in tests that open none.
    binder: OnceLock<crate::handoff::Binder>,
    /// The forwards open live now, by record.
    live: Mutex<HashMap<String, live::LiveListener>>,
    /// Whether a live forward the decisions allow failed to open, to retry at each poll.
    live_pending: AtomicBool,
    next_tunnel: AtomicU64,
    subject: Subject,
    upstream: U,
    /// The hosts that carry a bound credential (#832); None when the launch binds none.
    authority: Option<Authority>,
    report: Mutex<EgressReport>,
    /// Bumped at each change, so the writer rewrites the file only when something counted.
    changes: AtomicUsize,
    open: AtomicUsize,
    /// Held while the report file is written.
    writing: Mutex<()>,
}

/// The allowlist the proxy decides on, and the decisions that widened it.
struct Decided {
    allowlist: Allowlist,
    decisions: EgressDecisions,
}

/// One open tunnel: the entry that allowed it and both ends, to close on a revocation.
struct OpenTunnel {
    entry: AllowEntry,
    client: TcpStream,
    upstream: TcpStream,
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

/// The destinations listed in the requests file, and the revision it was last written at.
#[derive(Debug)]
struct Asked {
    destinations: BTreeSet<AskedDestination>,
    services: BTreeSet<AskedService>,
    revision: u64,
}

/// The decisions file's bytes; `None` when it is missing, or unreadable, which is the error
/// beside it.
fn read_decisions(path: &Path) -> (Option<Vec<u8>>, Option<std::io::Error>) {
    match std::fs::read(path) {
        Ok(text) => (Some(text), None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (None, None),
        Err(error) => (None, Some(error)),
    }
}

/// Where this launch's revisions start: the clock's nanoseconds, so a relaunch of the
/// session never writes a revision its last launch wrote.
fn launch_revision() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_nanos()).unwrap_or(u64::MAX)
        })
}

fn say_unreadable(path: &Path, error: &std::io::Error) {
    eprintln!(
        "torlek-sandbox: cannot read {}, so no decision applies: {error}",
        path.display()
    );
}

/// The owner's decisions in `path`: none when the file is missing, unreadable or malformed,
/// so what they allowed is refused again, never kept; an unreadable or malformed one is said.
pub fn decisions_at(path: &Path) -> EgressDecisions {
    let (read, problem) = read_decisions(path);
    if let Some(error) = &problem {
        say_unreadable(path, error);
    }
    decisions_in(path, read.as_deref())
}

/// The decisions `read` from `path` holds: none when the file is missing or malformed, so
/// the proxy fails closed.
fn decisions_in(path: &Path, read: Option<&[u8]>) -> EgressDecisions {
    let Some(text) = read else {
        return EgressDecisions::default();
    };
    serde_json::from_slice(text).unwrap_or_else(|error| {
        eprintln!(
            "torlek-sandbox: {} is malformed, so no decision applies: {error}",
            path.display()
        );
        EgressDecisions::default()
    })
}

impl<U: Upstream> Proxy<U> {
    pub fn new(
        allowlist: Allowlist,
        subject: Subject,
        upstream: U,
        authority: Option<Authority>,
    ) -> Arc<Proxy<U>> {
        Arc::new(Proxy {
            decided: RwLock::new(Arc::new(Decided {
                allowlist: allowlist.clone(),
                decisions: EgressDecisions::default(),
            })),
            allowlist,
            requests_file: OnceLock::new(),
            asked: Mutex::new(Asked {
                destinations: BTreeSet::new(),
                services: BTreeSet::new(),
                revision: launch_revision(),
            }),
            tunnels: Mutex::new(HashMap::new()),
            relays: Mutex::new(HashMap::new()),
            binder: OnceLock::new(),
            live: Mutex::new(HashMap::new()),
            live_pending: AtomicBool::new(false),
            next_tunnel: AtomicU64::new(0),
            report: Mutex::new(EgressReport::new(subject.clone())),
            subject,
            upstream,
            authority,
            changes: AtomicUsize::new(0),
            open: AtomicUsize::new(0),
            writing: Mutex::new(()),
        })
    }

    /// Lists the destinations refused for want of an entry in `path`, for the daemon to ask
    /// the owner about (#674).
    pub fn ask_through(&self, path: PathBuf) {
        let _ = self.requests_file.set(path);
    }

    /// Keeps the decisions current: `path` is read each [`DECISIONS_INTERVAL`] and applied
    /// when it changed, until the process ends. The first read always applies: a file
    /// replaced between the launch's read ([`decisions_at`]) and this thread's start is not
    /// missed.
    pub fn keep_decisions(self: &Arc<Self>, path: PathBuf) {
        let proxy = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("egress-decisions".to_owned())
            .spawn(move || {
                let mut applied: Option<Option<Vec<u8>>> = None;
                let mut said: Option<String> = None;
                loop {
                    std::thread::sleep(DECISIONS_INTERVAL);
                    let (read, problem) = read_decisions(&path);
                    // Said once while it lasts, not at every poll.
                    let problem_text = problem.as_ref().map(ToString::to_string);
                    if let Some(error) = &problem
                        && said != problem_text
                    {
                        say_unreadable(&path, error);
                    }
                    said = problem_text;
                    if applied.as_ref() == Some(&read) {
                        proxy.retry_live_forwards();
                    } else {
                        proxy.apply_decisions(decisions_in(&path, read.as_deref()));
                        applied = Some(read);
                    }
                }
            });
        drop(spawned);
    }

    /// Decides on the spec's allowlist with `decisions` from now on, and closes every open
    /// tunnel whose entry neither the spec nor `decisions` holds any more.
    pub fn apply_decisions(self: &Arc<Self>, decisions: EgressDecisions) {
        let decided = Arc::new(Decided {
            allowlist: self.allowlist.with_decisions(&decisions),
            decisions,
        });
        *self
            .decided
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::clone(&decided);
        // A decided destination is asked about again if a later decision takes it back. After
        // the swap: a refusal decided on the old decisions that lists it after this sees the
        // new ones in `ask` and does not.
        {
            let mut asked = self
                .asked
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            asked
                .destinations
                .retain(|asked| !decided.decisions.covers(&asked.host, asked.port));
            asked
                .services
                .retain(|asked| !decided.decisions.services.covers_record(asked));
        }
        self.close_revoked_relays(&decided.decisions.services);
        {
            let tunnels = self
                .tunnels
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let revoked = tunnels
                .values()
                .filter(|tunnel| !decided.allowlist.entries.contains(&tunnel.entry));
            for tunnel in revoked {
                let _ = tunnel.client.shutdown(Shutdown::Both);
                let _ = tunnel.upstream.shutdown(Shutdown::Both);
            }
        }
        // Last: a binder slow to answer delays no revocation.
        self.open_live_forwards(&decided.decisions.services);
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
        let (mut upstream, entry, expects_server_name) = opened;
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
            counters.source = Some(entry.source.to_string());
        });
        let Some(tunnel) = self.open_tunnel(entry, &client, &upstream) else {
            return;
        };
        let bound = self
            .authority
            .as_ref()
            .filter(|_| expects_server_name)
            .and_then(|authority| Some((authority, authority.host(&host)?)));
        if let Some((authority, bound)) = bound {
            self.intercept(
                client,
                upstream,
                first_bytes,
                &host,
                &destination,
                authority,
                bound,
            );
        } else if upstream.write_all(&first_bytes).is_ok() {
            let _ = client.set_read_timeout(None);
            self.splice(client, upstream, &destination, first_bytes.len() as u64);
        }
        self.tunnels
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&tunnel);
    }

    /// The decisions as they stand now.
    fn decided(&self) -> Arc<Decided> {
        Arc::clone(
            &self
                .decided
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    /// Registers the tunnel `entry` allowed, so a revocation closes it; None, closing both
    /// ends, when the entry was revoked since the connection was decided.
    fn open_tunnel(
        &self,
        entry: AllowEntry,
        client: &TcpStream,
        upstream: &TcpStream,
    ) -> Option<u64> {
        let (Ok(client_end), Ok(upstream_end)) = (client.try_clone(), upstream.try_clone()) else {
            return None;
        };
        let number = self.next_tunnel.fetch_add(1, Ordering::Relaxed);
        let mut tunnels = self
            .tunnels
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Checked under the lock `apply_decisions` closes tunnels under: a revocation is
        // either seen here or closes this tunnel there.
        if !self.decided().allowlist.entries.contains(&entry) {
            let _ = client.shutdown(Shutdown::Both);
            let _ = upstream.shutdown(Shutdown::Both);
            return None;
        }
        tunnels.insert(
            number,
            OpenTunnel {
                entry,
                client: client_end,
                upstream: upstream_end,
            },
        );
        Some(number)
    }

    /// The answer to a policy refusal of `host` on `port`. One the owner can lift asks them,
    /// through the requests file, unless they denied it; the answer says which.
    fn refused(&self, decided: &Decided, refusal: &EgressRefusal, host: &str, port: u16) -> Answer {
        if !refusal.is_askable() || !cockpit_sandbox::is_askable_destination(host, port) {
            return Answer::policy(refusal);
        }
        if let Some(request) = decided.decisions.denial(host, port) {
            return Answer::refused(
                "http_request_denied",
                format!(
                    "{} is not allowed: the owner denied network request {request}",
                    destination(host, port)
                ),
            );
        }
        // A proxy that files no requests (a check run's, #862) refuses as before #674.
        if self.requests_file.get().is_none() {
            return Answer::policy(refusal);
        }
        let asked = match self.ask(host, port) {
            // Read now: the daemon may have filed it since `decided` was taken.
            Ok(true) => match self.decided().decisions.asking(host, port) {
                Some(request) => format!(
                    "; the owner is asked in network request {request}: \
                     `torlek network allow {request}` lets it through"
                ),
                // The daemon files it within a second; the agent's next try names it.
                None => "; the owner is asked: `torlek network list` shows the request, \
                         `torlek network allow` lets it through"
                    .to_owned(),
            },
            Ok(false) => format!(
                "; the owner was not asked: {MAX_ASKED_DESTINATIONS} destinations were asked \
                 about already"
            ),
            Err(error) => format!("; the owner could not be asked: {error}"),
        };
        Answer::refused("http_request_denied", format!("{refusal}{asked}"))
    }

    /// Lists `host` on `port` in the requests file; false when the list is full. A
    /// destination listed already is not written again.
    fn ask(&self, host: &str, port: u16) -> Result<bool, String> {
        if self.requests_file.get().is_none() {
            return Err("this proxy files no requests".to_owned());
        }
        let mut asked = self
            .asked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let destination = AskedDestination {
            host: host.to_owned(),
            port,
        };
        // Decided since this connection was refused: nothing to ask.
        if asked.destinations.contains(&destination) || self.decided().decisions.covers(host, port)
        {
            return Ok(true);
        }
        if asked.destinations.len() >= MAX_ASKED_DESTINATIONS {
            return Ok(false);
        }
        asked.destinations.insert(destination.clone());
        let written = self.write_requests(&mut asked);
        if written.is_err() {
            // Not listed: the next refusal tries again instead of saying the owner is asked.
            asked.destinations.remove(&destination);
        }
        written.map(|()| true)
    }

    /// Writes what `asked` lists to the requests file at a new revision.
    fn write_requests(&self, asked: &mut Asked) -> Result<(), String> {
        let Some(path) = self.requests_file.get() else {
            return Err("this proxy files no requests".to_owned());
        };
        asked.revision += 1;
        let requests = EgressRequests {
            subject: self.subject.clone(),
            revision: asked.revision,
            asked: asked.destinations.iter().cloned().collect(),
            services: asked.services.iter().cloned().collect(),
        };
        serde_json::to_vec(&requests)
            .map_err(|error| error.to_string())
            .and_then(|text| {
                let staged = path.with_extension("json.staged");
                std::fs::write(&staged, text)
                    .and_then(|()| std::fs::rename(&staged, path))
                    .map_err(|error| format!("cannot write {}: {error}", path.display()))
            })
    }

    /// Decides, resolves, checks and connects: the upstream stream, the allowing entry's
    /// source and whether the server name is checked; or the answer to give.
    fn open_upstream(
        &self,
        host: &str,
        port: u16,
    ) -> Result<(TcpStream, AllowEntry, bool), Answer> {
        let decided = self.decided();
        let allowed = decided
            .allowlist
            .decide(host, port)
            .map_err(|refusal| self.refused(&decided, &refusal, host, port))?;
        let addresses = if allowed.is_name {
            let addresses = self.upstream.resolve(host, port).map_err(|error| {
                Answer::unreachable(
                    "502 Bad Gateway",
                    "dns_error",
                    format!("{} does not resolve: {error}", destination(host, port)),
                )
            })?;
            decided
                .allowlist
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
                    return Ok((stream, allowed.entry.clone(), allowed.expects_server_name));
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

    /// Ends the client's TLS with `bound`'s certificate, opens the proxy's own to `host`, and
    /// relays HTTP/1.1 between them with the credential added. A client that refuses the
    /// session CA's certificate, as one that pins its certificates does, is named with the
    /// host in the counters.
    #[allow(
        clippy::too_many_arguments,
        reason = "one tunnel's streams and names, each handed over once"
    )]
    fn intercept(
        &self,
        client: TcpStream,
        upstream: TcpStream,
        first_bytes: Vec<u8>,
        host: &str,
        destination: &str,
        authority: &Authority,
        bound: &BoundHost,
    ) {
        let _ = client.set_read_timeout(Some(HELLO_TIMEOUT));
        let Ok(connection) = rustls::ServerConnection::new(Arc::clone(&bound.server)) else {
            return;
        };
        let mut client = rustls::StreamOwned::new(connection, Prefixed::new(first_bytes, client));
        if let Err(error) = handshake(&mut client.conn, &mut client.sock) {
            let refused_by_client = received_alert(&error).is_some();
            let reason = match received_alert(&error) {
                Some(alert) => format!(
                    "the client refused the session CA's certificate for {host} ({alert:?}): a \
                     client that pins its certificates, or trusts neither SSL_CERT_FILE nor \
                     NODE_EXTRA_CA_CERTS, cannot use {} on {host}",
                    bound.credential.name
                ),
                // A client that pins may close without an alert; one that offers no
                // `http/1.1` gets the proxy's own.
                None => format!(
                    "the TLS handshake with the client for {host} failed ({error}): a client \
                     that pins its certificates, offers no http/1.1, or trusts neither \
                     SSL_CERT_FILE nor NODE_EXTRA_CA_CERTS, cannot use {} on {host}",
                    bound.credential.name
                ),
            };
            self.count(destination, |counters| {
                if refused_by_client {
                    counters.ca_refused += 1;
                }
                counters.last_reason = Some(reason);
            });
            return;
        }
        let _ = client.sock.stream.set_read_timeout(Some(IDLE_TIMEOUT));
        let mut client = Wire::new(client);
        let upstream = match self.open_tls(authority, host, upstream) {
            Ok(upstream) => upstream,
            Err((error, reason)) => {
                self.count(destination, |counters| {
                    counters.unreachable += 1;
                    counters.last_reason = Some(reason.clone());
                });
                // The client is owed an answer to its first request.
                if matches!(client_head(&mut client), Some(true)) {
                    let answer = Answer::unreachable("502 Bad Gateway", error, reason);
                    let _ = self.answer(client.get_mut(), destination, &answer);
                }
                close_tls(client.get_mut());
                return;
            }
        };
        let mut upstream = Wire::new(upstream);
        let tally = |event: Event| match event {
            Event::Bytes(direction, bytes) => self.add_bytes(destination, direction, bytes),
            Event::CredentialAdded => self.count(destination, |counters| {
                counters.credential_added += 1;
            }),
        };
        let ended = intercept::relay(&mut client, &mut upstream, host, &bound.credential, &tally);
        let answer = match ended {
            Ended::Closed => None,
            Ended::Refused { error, reason } => Some(Answer::refused(error, reason)),
            Ended::BadRequest(reason) => Some(Answer {
                status: "400 Bad Request",
                error: "http_request_error",
                reason,
                is_refusal: true,
            }),
            Ended::Broken(reason) => {
                self.count(destination, |counters| counters.last_reason = Some(reason));
                None
            }
        };
        if let Some(answer) = answer {
            self.count(destination, |counters| {
                counters.refused += 1;
                counters.last_reason = Some(answer.reason.clone());
            });
            let _ = self.answer(client.get_mut(), destination, &answer);
        }
        close_tls(client.get_mut());
        close_tls(upstream.get_mut());
    }

    /// The proxy's own TLS to `host`, verified against the host's roots; or the
    /// `Proxy-Status` error and reason for the `502`.
    fn open_tls(
        &self,
        authority: &Authority,
        host: &str,
        upstream: TcpStream,
    ) -> Result<rustls::StreamOwned<rustls::ClientConnection, TcpStream>, (&'static str, String)>
    {
        let failed = |error: String| {
            (
                "tls_protocol_error",
                format!("TLS with {host} failed: {error}"),
            )
        };
        let name = rustls::pki_types::ServerName::try_from(host.to_owned())
            .map_err(|error| failed(error.to_string()))?;
        let connection = rustls::ClientConnection::new(authority.upstream(), name)
            .map_err(|error| failed(error.to_string()))?;
        let _ = upstream.set_read_timeout(Some(CONNECT_TIMEOUT));
        let mut stream = rustls::StreamOwned::new(connection, upstream);
        if let Err(error) = handshake(&mut stream.conn, &mut stream.sock) {
            let is_certificate = matches!(
                rustls_error(&error),
                Some(rustls::Error::InvalidCertificate(_))
            );
            let (code, reason) = failed(error.to_string());
            let code = if is_certificate {
                "tls_certificate_error"
            } else {
                code
            };
            return Err((code, reason));
        }
        // A stalled upstream never holds the tunnel's thread longer than an idle client does.
        let _ = stream.sock.set_read_timeout(Some(IDLE_TIMEOUT));
        Ok(stream)
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
        client: &mut impl Write,
        destination: &str,
        answer: &Answer,
    ) -> std::io::Result<()> {
        let subject = match &self.subject {
            Subject::Session { session } => format!("session {session}"),
            Subject::Check { run, check } => format!("check run {run} of check {check}"),
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
        let details = header_text(&answer.reason);
        // Python, pip and Go print a failed `CONNECT`'s status line and `curl -v` shows it;
        // no client prints the body (#966).
        let response = format!(
            "HTTP/1.1 {}: torlek-sandbox: {details}\r\n\
             Proxy-Status: torlek-sandbox; error={}; details=\"{details}\"\r\n\
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

/// The bytes the proxy read from the client before it knew the tunnel is intercepted, its
/// ClientHello, then the client's stream: what the TLS server reads.
pub struct Prefixed {
    prefix: Vec<u8>,
    position: usize,
    pub stream: TcpStream,
}

impl Prefixed {
    fn new(prefix: Vec<u8>, stream: TcpStream) -> Prefixed {
        Prefixed {
            prefix,
            position: 0,
            stream,
        }
    }
}

impl Read for Prefixed {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let rest = &self.prefix[self.position..];
        if rest.is_empty() {
            return self.stream.read(buffer);
        }
        let read = rest.len().min(buffer.len());
        buffer[..read].copy_from_slice(&rest[..read]);
        self.position += read;
        Ok(read)
    }
}

impl Write for Prefixed {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.stream.write(bytes)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.stream.flush()
    }
}

/// Completes a TLS handshake on `stream`.
fn handshake<C, S, D>(connection: &mut C, stream: &mut S) -> std::io::Result<()>
where
    C: std::ops::DerefMut + std::ops::Deref<Target = rustls::ConnectionCommon<D>>,
    S: Read + Write,
    D: rustls::SideData,
{
    while connection.is_handshaking() {
        connection.complete_io(stream)?;
    }
    Ok(())
}

fn rustls_error(error: &std::io::Error) -> Option<&rustls::Error> {
    error.get_ref()?.downcast_ref::<rustls::Error>()
}

/// The alert the peer ended a handshake with, if it sent one.
fn received_alert(error: &std::io::Error) -> Option<rustls::AlertDescription> {
    match rustls_error(error)? {
        rustls::Error::AlertReceived(alert) => Some(*alert),
        _ => None,
    }
}

/// Whether the client sent a request head before it closed: Some(true) when it did.
fn client_head<C: Read + Write>(client: &mut Wire<C>) -> Option<bool> {
    client.head().ok().map(|head| head.is_some())
}

/// Ends one side's TLS with `close_notify`, as far as the peer still reads.
fn close_tls<C, S, D>(stream: &mut rustls::StreamOwned<C, S>)
where
    C: std::ops::DerefMut + std::ops::Deref<Target = rustls::ConnectionCommon<D>>,
    S: Read + Write,
    D: rustls::SideData,
{
    stream.conn.send_close_notify();
    let _ = stream.flush();
}

/// `text` as printable ASCII for a header or the status line, cut at [`MAX_HEADER_TEXT`]
/// bytes: the reason can quote what the client sent.
fn header_text(text: &str) -> String {
    let mut printable: String = text
        .chars()
        .map(|character| match character {
            '"' | '\\' => '\'',
            ' '..='~' => character,
            _ => '?',
        })
        .collect();
    if printable.len() > MAX_HEADER_TEXT {
        printable.truncate(MAX_HEADER_TEXT - 3);
        printable.push_str("...");
    }
    printable
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

    use cockpit_sandbox::{AllowEntry, AllowSource, AskingDestination, Target};

    use super::*;
    use crate::client_hello::tests::client_hello;

    mod interception;

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
        start_with(entries, upstream, None)
    }

    fn start_with(
        entries: Vec<AllowEntry>,
        upstream: Fake,
        authority: Option<Authority>,
    ) -> Running {
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
            authority,
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
        assert!(answer.starts_with("HTTP/1.1 403 Forbidden: "), "{answer}");
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

    fn decided_entry(host: &str, port: u16, request: i64) -> AllowEntry {
        AllowEntry {
            target: Target::Name(host.to_owned()),
            port,
            source: AllowSource::Request { request },
        }
    }

    fn asked_in(path: &Path) -> Vec<AskedDestination> {
        let text = std::fs::read(path).unwrap();
        serde_json::from_slice::<EgressRequests>(&text)
            .unwrap()
            .asked
    }

    /// #674: the refusal is at once and asks the owner, once per destination.
    #[test]
    fn a_refused_name_is_listed_once_for_the_owner_and_the_answer_says_so() {
        let folder = tempfile::tempdir().unwrap();
        let requests = folder.path().join(cockpit_sandbox::REQUESTS_FILE);
        let running = start(vec![name("api.anthropic.com", 443)], fake(&[], None));
        running.proxy.ask_through(requests.clone());

        let first = connect(running.address, "Unknown.Example.com:443", b"");
        let second = connect(running.address, "unknown.example.com:443", b"");

        for answer in [&first, &second] {
            assert!(answer.starts_with("HTTP/1.1 403 Forbidden: "), "{answer}");
            assert!(
                answer.contains("the owner is asked: `torlek network list` shows the request"),
                "{answer}"
            );
        }
        assert_eq!(
            asked_in(&requests),
            vec![AskedDestination {
                host: "unknown.example.com".to_owned(),
                port: 443,
            }]
        );
    }

    /// The answer to `CONNECT host:port` once the daemon filed it as network request 3.
    fn refused_while_asked(host: &str, port: u16) -> String {
        let folder = tempfile::tempdir().unwrap();
        let running = start(Vec::new(), fake(&[], None));
        running
            .proxy
            .ask_through(folder.path().join(cockpit_sandbox::REQUESTS_FILE));
        running.proxy.apply_decisions(EgressDecisions {
            asking: vec![AskingDestination {
                host: host.to_owned(),
                port,
                request: 3,
            }],
            ..EgressDecisions::default()
        });
        connect(running.address, &format!("{host}:{port}"), b"")
    }

    /// #966: once the daemon filed the request, the refusal names it and the owner's command.
    #[test]
    fn a_refusal_names_the_request_the_daemon_filed() {
        let answer = refused_while_asked("git.example.com", 22);

        assert!(
            answer.contains(
                "git.example.com:22 is not allowed: `claude` declares no hosts; the owner is \
                 asked in network request 3: `torlek network allow 3` lets it through"
            ),
            "{answer}"
        );
    }

    /// #966: Python, pip and Go print a failed `CONNECT`'s status line, `curl -v` shows it;
    /// no client prints the body.
    #[test]
    fn a_refusal_status_line_names_the_host_and_the_owners_command() {
        let answer = refused_while_asked("git.example.com", 22);

        let status_line = answer.lines().next().unwrap_or_default();
        assert_eq!(
            status_line,
            "HTTP/1.1 403 Forbidden: torlek-sandbox: git.example.com:22 is not allowed: \
             `claude` declares no hosts; the owner is asked in network request 3: \
             `torlek network allow 3` lets it through"
        );
    }

    /// #966: the owner's command ends the line, so a cut must never reach it.
    #[test]
    fn the_longest_dns_name_keeps_the_owners_command_on_the_status_line() {
        let host = format!(
            "{}.{}.{}.{}",
            "a".repeat(63),
            "b".repeat(63),
            "c".repeat(63),
            "d".repeat(61)
        );

        let answer = refused_while_asked(&host, 65_535);

        let status_line = answer.lines().next().unwrap_or_default();
        assert!(
            status_line.ends_with("`torlek network allow 3` lets it through"),
            "{status_line}"
        );
    }

    /// #966: a reason quoting what the client sent stays bounded printable text in the
    /// status line and the `Proxy-Status` header.
    #[test]
    fn a_reason_quoting_the_client_stays_short_and_printable_in_the_head() {
        let running = start(Vec::new(), fake(&[], None));
        let host = format!("\u{e9}{}", "x".repeat(4096));

        let answer = connect(running.address, &format!("{host}:443"), b"");

        let head = answer.split("\r\n\r\n").next().unwrap_or_default();
        for line in head.split("\r\n") {
            assert!(line.len() <= 640, "{} bytes: {line}", line.len());
            assert!(
                line.chars()
                    .all(|character| (' '..='~').contains(&character)),
                "{line}"
            );
        }
    }

    /// #674: a grant revoked after it covered a destination makes the next refusal ask again,
    /// in a file the daemon sees as changed.
    #[test]
    fn a_destination_refused_again_after_its_decision_was_taken_back_is_asked_again() {
        let folder = tempfile::tempdir().unwrap();
        let requests = folder.path().join(cockpit_sandbox::REQUESTS_FILE);
        let running = start(Vec::new(), fake(&[], None));
        running.proxy.ask_through(requests.clone());
        connect(running.address, "git.example.com:22", b"");
        let before = std::fs::read(&requests).unwrap();
        running.proxy.apply_decisions(EgressDecisions {
            allowed: vec![decided_entry("git.example.com", 22, 7)],
            denied: Vec::new(),
            asking: Vec::new(),
            services: Default::default(),
        });
        running.proxy.apply_decisions(EgressDecisions::default());

        let again = connect(running.address, "git.example.com:22", b"");

        assert!(again.contains("the owner is asked"), "{again}");
        assert_ne!(std::fs::read(&requests).unwrap(), before);
        assert_eq!(
            asked_in(&requests),
            vec![AskedDestination {
                host: "git.example.com".to_owned(),
                port: 22,
            }]
        );
    }

    /// #674: the daemon files only names of two labels or more; the refusal says so.
    #[test]
    fn a_single_label_name_asks_nobody() {
        let folder = tempfile::tempdir().unwrap();
        let requests = folder.path().join(cockpit_sandbox::REQUESTS_FILE);
        let running = start(Vec::new(), fake(&[], None));
        running.proxy.ask_through(requests.clone());

        let answer = connect(running.address, "intranet:443", b"");

        assert!(answer.starts_with("HTTP/1.1 403 "), "{answer}");
        assert!(!answer.contains("the owner is asked"), "{answer}");
        assert!(!requests.exists());
    }

    #[test]
    fn an_address_refusal_asks_nobody() {
        let folder = tempfile::tempdir().unwrap();
        let requests = folder.path().join(cockpit_sandbox::REQUESTS_FILE);
        let running = start(vec![name("api.anthropic.com", 443)], fake(&[], None));
        running.proxy.ask_through(requests.clone());

        let answer = connect(running.address, "93.184.215.14:443", b"");

        assert!(
            answer.contains("error=destination_ip_prohibited"),
            "{answer}"
        );
        assert!(!requests.exists());
    }

    #[test]
    fn a_destination_the_owner_allowed_is_reached_on_the_next_try() {
        let running = start(
            vec![name("api.anthropic.com", 443)],
            fake(
                &[("git.example.com", PUBLIC)],
                Some(echo_server(b"SSH-2.0-test\r\n")),
            ),
        );
        let refused = connect(running.address, "git.example.com:22", b"");
        assert!(refused.starts_with("HTTP/1.1 403 "), "{refused}");

        running.proxy.apply_decisions(EgressDecisions {
            allowed: vec![decided_entry("git.example.com", 22, 7)],
            denied: Vec::new(),
            asking: Vec::new(),
            services: Default::default(),
        });
        let answer = connect(running.address, "git.example.com:22", b"");

        assert_eq!(
            answer,
            "HTTP/1.1 200 Connection established\r\n\r\nSSH-2.0-test\r\n"
        );
        assert_eq!(
            running.proxy.report().destinations["git.example.com:22"].source,
            Some("network request 7, allowed for this session".to_owned())
        );
    }

    #[test]
    fn a_denied_destination_is_refused_naming_the_denial_without_asking_again() {
        let folder = tempfile::tempdir().unwrap();
        let requests = folder.path().join(cockpit_sandbox::REQUESTS_FILE);
        let running = start(vec![name("api.anthropic.com", 443)], fake(&[], None));
        running.proxy.ask_through(requests.clone());
        running.proxy.apply_decisions(EgressDecisions {
            allowed: Vec::new(),
            denied: vec![cockpit_sandbox::DeniedDestination {
                host: "unknown.example.com".to_owned(),
                port: 443,
                request: 3,
            }],
            asking: Vec::new(),
            services: Default::default(),
        });
        let started = Instant::now();

        let answer = connect(running.address, "unknown.example.com:443", b"");

        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(answer.starts_with("HTTP/1.1 403 Forbidden: "), "{answer}");
        assert!(
            answer.contains(
                "unknown.example.com:443 is not allowed: the owner denied network request 3"
            ),
            "{answer}"
        );
        assert!(!requests.exists());
    }

    /// #674: revoking what allowed a tunnel closes it, not only the next connection.
    #[test]
    fn a_revoked_decision_closes_the_tunnels_it_allowed() {
        let running = start(
            Vec::new(),
            fake(&[("git.example.com", PUBLIC)], Some(echo_server(b""))),
        );
        running.proxy.apply_decisions(EgressDecisions {
            allowed: vec![decided_entry("git.example.com", 22, 7)],
            denied: Vec::new(),
            asking: Vec::new(),
            services: Default::default(),
        });
        let mut client = TcpStream::connect(running.address).unwrap();
        client
            .write_all(b"CONNECT git.example.com:22 HTTP/1.1\r\n\r\n")
            .unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut established = [0_u8; 39];
        client.read_exact(&mut established).unwrap();
        assert_eq!(&established, b"HTTP/1.1 200 Connection established\r\n\r\n");
        client.write_all(b"ping").unwrap();
        let mut echoed = [0_u8; 4];
        client.read_exact(&mut echoed).unwrap();

        running.proxy.apply_decisions(EgressDecisions::default());

        let mut rest = Vec::new();
        let closed = client.read_to_end(&mut rest);
        assert!(closed.is_ok(), "the tunnel stayed open: {closed:?}");
        assert!(rest.is_empty(), "{rest:?}");
        let answer = connect(running.address, "git.example.com:22", b"");
        assert!(answer.starts_with("HTTP/1.1 403 "), "{answer}");
    }

    #[test]
    fn a_malformed_decisions_file_applies_no_decision() {
        let folder = tempfile::tempdir().unwrap();
        let decisions = folder.path().join(cockpit_sandbox::DECISIONS_FILE);
        let running = start(
            Vec::new(),
            fake(&[("git.example.com", PUBLIC)], Some(echo_server(b"hi"))),
        );
        let allowed = EgressDecisions {
            allowed: vec![decided_entry("git.example.com", 22, 7)],
            denied: Vec::new(),
            asking: Vec::new(),
            services: Default::default(),
        };
        std::fs::write(&decisions, serde_json::to_vec(&allowed).unwrap()).unwrap();
        running.proxy.apply_decisions(decisions_at(&decisions));
        assert!(connect(running.address, "git.example.com:22", b"").starts_with("HTTP/1.1 200 "));

        std::fs::write(&decisions, b"{").unwrap();
        running.proxy.apply_decisions(decisions_at(&decisions));

        let answer = connect(running.address, "git.example.com:22", b"");
        assert!(answer.starts_with("HTTP/1.1 403 "), "{answer}");
    }

    #[test]
    fn a_changed_decisions_file_applies_while_the_proxy_runs() {
        let folder = tempfile::tempdir().unwrap();
        let decisions = folder.path().join(cockpit_sandbox::DECISIONS_FILE);
        let running = start(
            Vec::new(),
            fake(&[("git.example.com", PUBLIC)], Some(echo_server(b"hi"))),
        );
        running.proxy.keep_decisions(decisions.clone());
        let allowed = EgressDecisions {
            allowed: vec![decided_entry("git.example.com", 22, 7)],
            denied: Vec::new(),
            asking: Vec::new(),
            services: Default::default(),
        };

        std::fs::write(&decisions, serde_json::to_vec(&allowed).unwrap()).unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut answer = connect(running.address, "git.example.com:22", b"");
        while !answer.starts_with("HTTP/1.1 200 ") && Instant::now() < deadline {
            std::thread::sleep(DECISIONS_INTERVAL);
            answer = connect(running.address, "git.example.com:22", b"");
        }
        assert!(answer.starts_with("HTTP/1.1 200 "), "{answer}");
    }
}
