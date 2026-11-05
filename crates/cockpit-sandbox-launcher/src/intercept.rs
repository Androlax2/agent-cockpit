//! What crosses an intercepted tunnel (#832): HTTP/1.1, one request and its response at a
//! time, each request's head read, checked and rewritten before it goes upstream; bodies and
//! responses pass as they arrive, framed but unchanged.
//!
//! A request names the tunnel's host or is refused, so a credential never reaches another
//! service behind the same front end. A protocol upgrade is refused: a WebSocket on a
//! credential host is not carried yet. Any request or response whose framing is ambiguous
//! (both `Transfer-Encoding` and `Content-Length`, differing lengths, a coding other than
//! chunked last) ends the connection rather than be guessed at.

use std::io::{self, Read, Write};

use cockpit_sandbox::CredentialScheme;

/// The longest request or response head read.
const MAX_HEAD_BYTES: usize = 64 * 1024;
/// The most header fields one head holds.
const MAX_HEADERS: usize = 128;
/// The longest chunk-size or trailer line.
const MAX_LINE_BYTES: usize = 8 * 1024;

/// A credential the proxy adds on one host.
pub struct Credential {
    /// What the proxy's answers call it.
    pub name: String,
    pub scheme: CredentialScheme,
    value: String,
}

impl Credential {
    /// Refuses a value no header can carry: one with a control character.
    pub fn new(
        name: String,
        scheme: CredentialScheme,
        value: String,
    ) -> Result<Credential, String> {
        if value.is_empty() || value.chars().any(char::is_control) {
            return Err(format!(
                "the value of {name} is empty or holds a control character"
            ));
        }
        Ok(Credential {
            name,
            scheme,
            value,
        })
    }
}

/// Which way bytes go: out of the sandbox, or into it.
#[derive(Clone, Copy)]
pub enum Direction {
    Out,
    In,
}

/// What a relay reports as it goes.
pub enum Event {
    /// Bytes of HTTP passed one way.
    Bytes(Direction, u64),
    /// A request went upstream with the credential added.
    CredentialAdded,
}

/// How an intercepted connection ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Ended {
    /// Either side closed, or the exchange asked to close.
    Closed,
    /// A request was refused before anything of it went upstream: answer it, then close.
    Refused { error: &'static str, reason: String },
    /// A request's head or framing is not HTTP/1.1 the proxy carries, and nothing of it went
    /// upstream: answer `400`, then close.
    BadRequest(String),
    /// The exchange broke halfway: close without an answer.
    Broken(String),
}

/// A stream and the bytes read from it but not yet passed on.
pub struct Wire<T> {
    stream: T,
    pending: Vec<u8>,
}

impl<T: Read + Write> Wire<T> {
    pub fn new(stream: T) -> Wire<T> {
        Wire {
            stream,
            pending: Vec::new(),
        }
    }

    pub fn get_mut(&mut self) -> &mut T {
        &mut self.stream
    }

    /// Reads more; false at the end of the stream.
    fn fill(&mut self) -> io::Result<bool> {
        let mut chunk = [0_u8; 16 * 1024];
        let read = self.stream.read(&mut chunk)?;
        self.pending.extend_from_slice(&chunk[..read]);
        Ok(read > 0)
    }

    /// The next head, through its blank line; None when the stream ends, or goes idle past
    /// its timeout, before a byte of it.
    pub fn head(&mut self) -> Result<Option<Vec<u8>>, String> {
        loop {
            if let Some(end) = find(&self.pending, b"\r\n\r\n") {
                let rest = self.pending.split_off(end + 4);
                return Ok(Some(std::mem::replace(&mut self.pending, rest)));
            }
            if self.pending.len() > MAX_HEAD_BYTES {
                return Err("a head is longer than 64 KiB".to_owned());
            }
            match self.fill() {
                Ok(true) => {}
                Ok(false) if self.pending.is_empty() => return Ok(None),
                Err(error) if self.pending.is_empty() && is_quiet_end(&error) => return Ok(None),
                Ok(false) => return Err("the stream ended inside a head".to_owned()),
                Err(error) => return Err(format!("a head did not arrive: {error}")),
            }
        }
    }

    /// One line through its CRLF: a chunk size or a trailer.
    fn line(&mut self) -> Result<Vec<u8>, String> {
        loop {
            if let Some(end) = find(&self.pending, b"\r\n") {
                let rest = self.pending.split_off(end + 2);
                return Ok(std::mem::replace(&mut self.pending, rest));
            }
            if self.pending.len() > MAX_LINE_BYTES {
                return Err("a chunk line is longer than 8 KiB".to_owned());
            }
            match self.fill() {
                Ok(true) => {}
                Ok(false) => return Err("the stream ended inside a chunked body".to_owned()),
                Err(error) => return Err(format!("a chunked body broke off: {error}")),
            }
        }
    }

    /// Passes the next `length` bytes to `to`.
    fn copy_exact(&mut self, to: &mut impl Write, mut length: u64) -> Result<(), String> {
        while length > 0 {
            if self.pending.is_empty() && !self.fill().map_err(broke_off)? {
                return Err("the stream ended inside a body".to_owned());
            }
            let take = usize::try_from(length)
                .unwrap_or(usize::MAX)
                .min(self.pending.len());
            to.write_all(&self.pending[..take]).map_err(broke_off)?;
            self.pending.drain(..take);
            length -= take as u64;
        }
        Ok(())
    }

    /// Passes everything to `to` until the stream ends.
    fn copy_to_end(&mut self, to: &mut impl Write) -> Result<(), String> {
        loop {
            if !self.pending.is_empty() {
                to.write_all(&self.pending).map_err(broke_off)?;
                to.flush().map_err(broke_off)?;
                self.pending.clear();
            }
            match self.fill() {
                Ok(true) => {}
                Ok(false) => return Ok(()),
                Err(error) if is_quiet_end(&error) => return Ok(()),
                Err(error) => return Err(broke_off(error)),
            }
        }
    }

    /// Passes one chunked body, its trailers included, to `to`.
    fn copy_chunked(&mut self, to: &mut impl Write) -> Result<(), String> {
        loop {
            let line = self.line()?;
            to.write_all(&line).map_err(broke_off)?;
            let size = chunk_size(&line)?;
            if size == 0 {
                loop {
                    let trailer = self.line()?;
                    to.write_all(&trailer).map_err(broke_off)?;
                    if trailer == b"\r\n" {
                        return to.flush().map_err(broke_off);
                    }
                }
            }
            self.copy_exact(to, size)?;
            let end = self.line()?;
            if end != b"\r\n" {
                return Err("a chunk does not end where its size says".to_owned());
            }
            to.write_all(&end).map_err(broke_off)?;
            to.flush().map_err(broke_off)?;
        }
    }
}

fn broke_off(error: io::Error) -> String {
    format!("the connection broke off: {error}")
}

/// An end a client or server gives without notice: closed without TLS's `close_notify`,
/// reset, or idle past its timeout.
fn is_quiet_end(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::UnexpectedEof
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::BrokenPipe
            | io::ErrorKind::TimedOut
            | io::ErrorKind::WouldBlock
    )
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// A writer that reports what passes through it.
struct Counted<'a, W> {
    inner: &'a mut W,
    direction: Direction,
    tally: &'a dyn Fn(Event),
}

impl<W: Write> Write for Counted<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(bytes)?;
        (self.tally)(Event::Bytes(self.direction, written as u64));
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// One parsed head's header fields, names as sent.
type Headers = Vec<(String, Vec<u8>)>;

struct Request {
    method: String,
    target: String,
    /// The minor version: `HTTP/1.0` or `HTTP/1.1`.
    version: u8,
    headers: Headers,
}

struct Response {
    status: u16,
    version: u8,
    headers: Headers,
}

fn parse_request(head: &[u8]) -> Result<Request, String> {
    let mut fields = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut request = httparse::Request::new(&mut fields);
    match request.parse(head) {
        Ok(httparse::Status::Complete(_)) => {}
        Ok(httparse::Status::Partial) => return Err("the request head is incomplete".to_owned()),
        Err(error) => return Err(format!("the request head is malformed: {error}")),
    }
    Ok(Request {
        method: request.method.unwrap_or_default().to_owned(),
        target: request.path.unwrap_or_default().to_owned(),
        version: request.version.unwrap_or_default(),
        headers: owned(request.headers),
    })
}

fn parse_response(head: &[u8]) -> Result<Response, String> {
    let mut fields = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut response = httparse::Response::new(&mut fields);
    match response.parse(head) {
        Ok(httparse::Status::Complete(_)) => {}
        Ok(httparse::Status::Partial) => {
            return Err("the upstream's response head is incomplete".to_owned());
        }
        Err(error) => {
            return Err(format!(
                "the upstream's response head is malformed: {error}"
            ));
        }
    }
    Ok(Response {
        status: response.code.unwrap_or_default(),
        version: response.version.unwrap_or_default(),
        headers: owned(response.headers),
    })
}

fn owned(fields: &[httparse::Header<'_>]) -> Headers {
    fields
        .iter()
        .map(|field| (field.name.to_owned(), field.value.to_vec()))
        .collect()
}

fn values<'a>(headers: &'a Headers, name: &'a str) -> impl Iterator<Item = &'a [u8]> + 'a {
    headers
        .iter()
        .filter(move |(field, _)| field.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_slice())
}

/// Whether the comma-separated field `name` holds `token`, in any case.
fn has_token(headers: &Headers, name: &str, token: &str) -> bool {
    values(headers, name)
        .flat_map(|value| value.split(|byte| *byte == b','))
        .any(|item| item.trim_ascii().eq_ignore_ascii_case(token.as_bytes()))
}

/// How a body is delimited.
#[derive(Debug, PartialEq, Eq)]
enum Body {
    None,
    Length(u64),
    Chunked,
    UntilClose,
}

/// Whether `Transfer-Encoding` ends with `chunked`; None without the field.
fn ends_chunked(headers: &Headers) -> Option<bool> {
    values(headers, "transfer-encoding").next()?;
    let codings: Vec<&[u8]> = values(headers, "transfer-encoding")
        .flat_map(|value| value.split(|byte| *byte == b','))
        .map(<[u8]>::trim_ascii)
        .filter(|coding| !coding.is_empty())
        .collect();
    Some(
        codings
            .last()
            .is_some_and(|coding| coding.eq_ignore_ascii_case(b"chunked")),
    )
}

/// The one length every `Content-Length` value states; None without the field.
fn content_length(headers: &Headers) -> Result<Option<u64>, String> {
    let mut length = None;
    for item in
        values(headers, "content-length").flat_map(|value| value.split(|byte| *byte == b','))
    {
        let item = item.trim_ascii();
        let parsed = std::str::from_utf8(item)
            .ok()
            .filter(|text| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()))
            .and_then(|text| text.parse::<u64>().ok())
            .ok_or("a Content-Length is not a number")?;
        match length {
            Some(earlier) if earlier != parsed => {
                return Err("two Content-Length values differ".to_owned());
            }
            _ => length = Some(parsed),
        }
    }
    Ok(length)
}

fn request_body(request: &Request) -> Result<Body, String> {
    let length = content_length(&request.headers)?;
    match (ends_chunked(&request.headers), length) {
        (Some(_), Some(_)) => {
            Err("a request carries both Transfer-Encoding and Content-Length".to_owned())
        }
        (Some(true), None) => Ok(Body::Chunked),
        (Some(false), None) => {
            Err("a request's Transfer-Encoding does not end with chunked".to_owned())
        }
        (None, Some(0) | None) => Ok(Body::None),
        (None, Some(length)) => Ok(Body::Length(length)),
    }
}

fn response_body(request: &Request, response: &Response) -> Result<Body, String> {
    let has_no_body = request.method == "HEAD"
        || (100..200).contains(&response.status)
        || response.status == 204
        || response.status == 304;
    if has_no_body {
        return Ok(Body::None);
    }
    match ends_chunked(&response.headers) {
        Some(true) => return Ok(Body::Chunked),
        Some(false) => return Ok(Body::UntilClose),
        None => {}
    }
    Ok(match content_length(&response.headers)? {
        Some(0) => Body::None,
        Some(length) => Body::Length(length),
        None => Body::UntilClose,
    })
}

fn chunk_size(line: &[u8]) -> Result<u64, String> {
    let line = line.strip_suffix(b"\r\n").unwrap_or(line);
    let size = line
        .split(|byte| *byte == b';')
        .next()
        .unwrap_or_default()
        .trim_ascii();
    let is_hex = !size.is_empty() && size.len() <= 15 && size.iter().all(u8::is_ascii_hexdigit);
    if !is_hex {
        return Err("a chunk size is not a hexadecimal number".to_owned());
    }
    let text = std::str::from_utf8(size).map_err(|_| "a chunk size is not ASCII".to_owned())?;
    u64::from_str_radix(text, 16).map_err(|error| format!("a chunk size is invalid: {error}"))
}

/// What the proxy does with one request on a credential host.
#[derive(Debug, PartialEq, Eq)]
enum Action {
    /// Sends it with this `Authorization` value in place of the client's.
    AddCredential(String),
    /// Sends it as the client wrote it.
    PassUnchanged,
    Refuse(String),
}

/// What `credential`'s scheme makes of a request for `target`.
fn decide(credential: &Credential, target: &str) -> Action {
    match &credential.scheme {
        CredentialScheme::Bearer => Action::AddCredential(format!("Bearer {}", credential.value)),
        CredentialScheme::GitReads { username } => {
            let decoded = percent_decoded(target).to_ascii_lowercase();
            if decoded.contains("git-receive-pack") {
                return Action::Refuse(
                    "a Git push over HTTPS is refused inside the sandbox; pushes go through \
                     `torlek push`, which the daemon runs (#857)"
                        .to_owned(),
                );
            }
            if is_upload_pack(&decoded) {
                let pair = format!("{username}:{}", credential.value);
                Action::AddCredential(format!("Basic {}", base64(pair.as_bytes())))
            } else {
                Action::PassUnchanged
            }
        }
    }
}

/// A Git fetch: `.../info/refs?service=git-upload-pack` or `.../git-upload-pack`, as a
/// decoded, lower-case target.
fn is_upload_pack(target: &str) -> bool {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let path = path.trim_end_matches('/');
    path.ends_with("/git-upload-pack")
        || (path.ends_with("/info/refs")
            && query
                .split('&')
                .any(|parameter| parameter == "service=git-upload-pack"))
}

/// `%XX` sequences decoded, so an encoded name matches as the server would read it; any
/// other `%` is kept.
fn percent_decoded(target: &str) -> String {
    let bytes = target.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let hex = bytes
            .get(index + 1..index + 3)
            .and_then(|pair| std::str::from_utf8(pair).ok())
            .and_then(|pair| u8::from_str_radix(pair, 16).ok());
        match (bytes[index], hex) {
            (b'%', Some(byte)) => {
                decoded.push(byte);
                index += 3;
            }
            (byte, _) => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for group in bytes.chunks(3) {
        let joined = group
            .iter()
            .enumerate()
            .fold(0_u32, |joined, (index, byte)| {
                joined | u32::from(*byte) << (16 - 8 * index)
            });
        for position in 0..4 {
            if position <= group.len() {
                let index = (joined >> (18 - 6 * position)) & 0x3f;
                encoded.push(char::from(ALPHABET[index as usize]));
            } else {
                encoded.push('=');
            }
        }
    }
    encoded
}

/// Whether the request names `host`, the tunnel's: its `Host` field, and its target's
/// authority when it has one. A name it does not hold is refused, so the credential reaches
/// no other service behind the same front end.
fn names_host(request: &Request, host: &str) -> Result<(), String> {
    let is_host = |authority: &[u8]| {
        let authority = authority.trim_ascii();
        let authority = authority.strip_suffix(b":443").unwrap_or(authority);
        authority.eq_ignore_ascii_case(host.as_bytes())
    };
    let hosts: Vec<&[u8]> = values(&request.headers, "host").collect();
    let [field] = hosts.as_slice() else {
        return Err(format!(
            "a request inside the tunnel to {host} must carry one Host field"
        ));
    };
    if !is_host(field) {
        return Err(format!(
            "a request inside the tunnel to {host} names the host {}",
            String::from_utf8_lossy(field)
        ));
    }
    let lowered = request.target.to_ascii_lowercase();
    if let Some(rest) = lowered
        .strip_prefix("https://")
        .or_else(|| lowered.strip_prefix("http://"))
    {
        let authority = rest.split(['/', '?']).next().unwrap_or_default();
        if !is_host(authority.as_bytes()) {
            return Err(format!(
                "a request inside the tunnel to {host} targets {}",
                request.target
            ));
        }
    }
    Ok(())
}

/// The head that goes upstream: the client's, `Expect` dropped when the proxy answered it,
/// and the credential in place of the client's `Authorization` when added.
fn rewritten_head(request: &Request, authorization: Option<&str>, drops_expect: bool) -> Vec<u8> {
    let mut head = format!(
        "{} {} HTTP/1.{}\r\n",
        request.method, request.target, request.version
    )
    .into_bytes();
    for (name, value) in &request.headers {
        let is_replaced = authorization.is_some() && name.eq_ignore_ascii_case("authorization");
        let is_answered = drops_expect && name.eq_ignore_ascii_case("expect");
        if is_replaced || is_answered {
            continue;
        }
        head.extend_from_slice(name.as_bytes());
        head.extend_from_slice(b": ");
        head.extend_from_slice(value);
        head.extend_from_slice(b"\r\n");
    }
    if let Some(authorization) = authorization {
        head.extend_from_slice(b"Authorization: ");
        head.extend_from_slice(authorization.as_bytes());
        head.extend_from_slice(b"\r\n");
    }
    head.extend_from_slice(b"\r\n");
    head
}

/// Carries requests from `client` to `upstream` and their responses back, adding
/// `credential` as its scheme says, until either side closes or a request is refused.
pub fn relay<C: Read + Write, U: Read + Write>(
    client: &mut Wire<C>,
    upstream: &mut Wire<U>,
    host: &str,
    credential: &Credential,
    tally: &dyn Fn(Event),
) -> Ended {
    loop {
        let head = match client.head() {
            Ok(Some(head)) => head,
            Ok(None) => return Ended::Closed,
            Err(reason) => return Ended::BadRequest(reason),
        };
        let request = match parse_request(&head) {
            Ok(request) => request,
            Err(reason) => return Ended::BadRequest(reason),
        };
        if let Err(reason) = names_host(&request, host) {
            return Ended::Refused {
                error: "http_request_denied",
                reason,
            };
        }
        if values(&request.headers, "upgrade").next().is_some() {
            return Ended::Refused {
                error: "http_request_denied",
                reason: format!(
                    "a protocol upgrade is not carried on {host}, which carries {}",
                    credential.name
                ),
            };
        }
        let authorization = match decide(credential, &request.target) {
            Action::AddCredential(value) => Some(value),
            Action::PassUnchanged => None,
            Action::Refuse(reason) => {
                return Ended::Refused {
                    error: "http_request_denied",
                    reason,
                };
            }
        };
        let body = match request_body(&request) {
            Ok(body) => body,
            Err(reason) => return Ended::BadRequest(reason),
        };
        let expects_continue = has_token(&request.headers, "expect", "100-continue");
        let closes_after =
            request.version == 0 || has_token(&request.headers, "connection", "close");
        let exchanged = exchange(
            client,
            upstream,
            &request,
            body,
            authorization.as_deref(),
            expects_continue,
            tally,
        );
        match exchanged {
            Ok(Continue::Next) if !closes_after => {}
            Ok(_) => return Ended::Closed,
            Err(reason) => return Ended::Broken(reason),
        }
    }
}

enum Continue {
    Next,
    Close,
}

/// Sends one request upstream and its response back.
fn exchange<C: Read + Write, U: Read + Write>(
    client: &mut Wire<C>,
    upstream: &mut Wire<U>,
    request: &Request,
    body: Body,
    authorization: Option<&str>,
    expects_continue: bool,
    tally: &dyn Fn(Event),
) -> Result<Continue, String> {
    {
        let mut out = Counted {
            inner: upstream.get_mut(),
            direction: Direction::Out,
            tally,
        };
        out.write_all(&rewritten_head(request, authorization, expects_continue))
            .map_err(broke_off)?;
        if authorization.is_some() {
            tally(Event::CredentialAdded);
        }
        if expects_continue {
            // The proxy answers the expectation itself, so the body follows without a round
            // trip it would otherwise have to interleave.
            let mut back = Counted {
                inner: client.get_mut(),
                direction: Direction::In,
                tally,
            };
            back.write_all(b"HTTP/1.1 100 Continue\r\n\r\n")
                .map_err(broke_off)?;
            back.flush().map_err(broke_off)?;
        }
        match body {
            Body::None => {}
            Body::Length(length) => client.copy_exact(&mut out, length)?,
            Body::Chunked => client.copy_chunked(&mut out)?,
            Body::UntilClose => unreachable!("a request body is never delimited by its close"),
        }
        out.flush().map_err(broke_off)?;
    }
    let mut back = Counted {
        inner: client.get_mut(),
        direction: Direction::In,
        tally,
    };
    let response = loop {
        let head = upstream
            .head()?
            .ok_or("the upstream closed before it answered")?;
        let response = parse_response(&head)?;
        back.write_all(&head).map_err(broke_off)?;
        if response.status == 101 {
            return Err("the upstream switched protocols unasked".to_owned());
        }
        if !(100..200).contains(&response.status) {
            break response;
        }
        back.flush().map_err(broke_off)?;
    };
    let body = response_body(request, &response)?;
    match body {
        Body::None => {}
        Body::Length(length) => upstream.copy_exact(&mut back, length)?,
        Body::Chunked => upstream.copy_chunked(&mut back)?,
        Body::UntilClose => {
            upstream.copy_to_end(&mut back)?;
            return Ok(Continue::Close);
        }
    }
    back.flush().map_err(broke_off)?;
    if response.version == 0 || has_token(&response.headers, "connection", "close") {
        return Ok(Continue::Close);
    }
    Ok(Continue::Next)
}

#[cfg(test)]
mod tests;
