//! Just enough HTTP/1.1 to talk to a session's OpenCode server on the loopback interface: one
//! request per connection (`Connection: close`), basic authentication, JSON bodies, and a
//! response read whole, with a length or chunked. No proxy, no TLS, no redirect.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::time::{Duration, Instant};

/// The largest response read; a command list or a provider catalog is far below it.
const MAX_RESPONSE_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Method {
    Get,
    Post,
}

impl Method {
    fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
        }
    }
}

pub(crate) struct Request<'request> {
    pub method: Method,
    /// Starts with `/`; the caller builds it from validated parts only.
    pub path: &'request str,
    pub body: Option<&'request str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Response {
    pub status: u16,
    pub body: String,
}

/// Sends `request` to `127.0.0.1:port` as `user:password` and reads its whole response, all
/// within `deadline`. The reason when it fails.
pub(crate) fn send(
    port: u16,
    user: &str,
    password: &str,
    request: &Request<'_>,
    deadline: Duration,
) -> Result<Response, String> {
    let ends_at = Instant::now() + deadline;
    let remaining = || {
        ends_at
            .checked_duration_since(Instant::now())
            .filter(|left| !left.is_zero())
            .ok_or_else(|| format!("no answer within {} ms", deadline.as_millis()))
    };
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut stream = TcpStream::connect_timeout(&address, remaining()?)
        .map_err(|error| format!("cannot connect to {address}: {error}"))?;
    stream
        .set_write_timeout(Some(remaining()?))
        .map_err(|error| error.to_string())?;
    stream
        .write_all(&encode(port, user, password, request))
        .map_err(|error| format!("cannot send the request: {error}"))?;
    let mut raw = Vec::new();
    let mut limited = (&stream).take(MAX_RESPONSE_BYTES + 1);
    let mut buffer = [0u8; 8192];
    loop {
        stream
            .set_read_timeout(Some(remaining()?))
            .map_err(|error| error.to_string())?;
        match limited.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => raw.extend_from_slice(&buffer[..read]),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                return Err(format!("no answer within {} ms", deadline.as_millis()));
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(format!("cannot read the answer: {error}")),
        }
    }
    if raw.len() as u64 > MAX_RESPONSE_BYTES {
        return Err(format!("the answer is over {MAX_RESPONSE_BYTES} bytes"));
    }
    parse(&raw)
}

fn encode(port: u16, user: &str, password: &str, request: &Request<'_>) -> Vec<u8> {
    let credentials = base64(format!("{user}:{password}").as_bytes());
    let mut head = format!(
        "{} {} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Basic {credentials}\r\n\
         Accept: application/json\r\nConnection: close\r\n",
        request.method.as_str(),
        request.path,
    );
    let body = request.body.unwrap_or_default();
    if request.body.is_some() {
        head.push_str("Content-Type: application/json\r\n");
    }
    if request.body.is_some() || request.method == Method::Post {
        head.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    head.push_str("\r\n");
    let mut bytes = head.into_bytes();
    bytes.extend_from_slice(body.as_bytes());
    bytes
}

/// A whole response: its status and its body, decoded from chunks when it came in chunks.
fn parse(raw: &[u8]) -> Result<Response, String> {
    let malformed = |what: &str| format!("a malformed answer: {what}");
    let split = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| malformed("no end of headers"))?;
    let head = std::str::from_utf8(&raw[..split]).map_err(|_| malformed("headers not UTF-8"))?;
    let rest = &raw[split + 4..];
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| {
            let mut words = line.split(' ');
            let version = words.next()?;
            version.starts_with("HTTP/1.").then_some(())?;
            words.next()?.parse::<u16>().ok()
        })
        .ok_or_else(|| malformed("no status line"))?;
    let header = |name: &str| {
        lines.clone().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.trim()
                .eq_ignore_ascii_case(name)
                .then(|| value.trim().to_owned())
        })
    };
    let is_chunked = header("transfer-encoding")
        .is_some_and(|value| value.to_ascii_lowercase().contains("chunked"));
    let body = if is_chunked {
        dechunk(rest).ok_or_else(|| malformed("broken chunks"))?
    } else if let Some(length) = header("content-length") {
        let length: usize = length
            .parse()
            .map_err(|_| malformed("a bad Content-Length"))?;
        rest.get(..length)
            .ok_or_else(|| malformed("shorter than its Content-Length"))?
            .to_vec()
    } else {
        rest.to_vec()
    };
    let body = String::from_utf8(body).map_err(|_| malformed("a body not UTF-8"))?;
    Ok(Response { status, body })
}

fn dechunk(mut rest: &[u8]) -> Option<Vec<u8>> {
    let mut body = Vec::new();
    loop {
        let line_end = rest.windows(2).position(|window| window == b"\r\n")?;
        let size_line = std::str::from_utf8(&rest[..line_end]).ok()?;
        let size_text = size_line.split(';').next()?.trim();
        let size = usize::from_str_radix(size_text, 16).ok()?;
        rest = &rest[line_end + 2..];
        if size == 0 {
            return Some(body);
        }
        body.extend_from_slice(rest.get(..size)?);
        rest = rest.get(size..)?.strip_prefix(b"\r\n")?;
    }
}

/// Standard base64 with padding, for the `Authorization` header.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for group in bytes.chunks(3) {
        let triple = group.iter().enumerate().fold(0u32, |value, (index, byte)| {
            value | u32::from(*byte) << (16 - 8 * index)
        });
        for position in 0..4 {
            if position <= group.len() {
                let index = (triple >> (18 - 6 * position)) & 0x3f;
                encoded.push(char::from(ALPHABET[index as usize]));
            } else {
                encoded.push('=');
            }
        }
    }
    encoded
}
