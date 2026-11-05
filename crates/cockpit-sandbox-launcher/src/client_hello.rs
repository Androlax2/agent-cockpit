//! The server name of a TLS ClientHello (RFC 8446 4.1.2, RFC 6066 3), read from the first
//! bytes a client sends through a tunnel to port 443. Only read: the bytes pass on unchanged.
//! An Encrypted Client Hello (RFC 9849) shows its outer name here, which the `CONNECT` check
//! then compares like any other.

/// The longest ClientHello read before giving up: post-quantum key shares make it a few
/// kilobytes; a record holds at most 16 KiB and a hello may span several.
pub const MAX_HELLO_BYTES: usize = 64 * 1024;

const HANDSHAKE_RECORD: u8 = 22;
const CLIENT_HELLO: u8 = 1;
const SERVER_NAME_EXTENSION: u16 = 0;
const HOST_NAME_TYPE: u8 = 0;
const RECORD_HEADER_BYTES: usize = 5;

#[derive(Debug, PartialEq, Eq)]
pub enum ServerName {
    /// The host name, lower case, without a trailing dot.
    Named(String),
    /// A complete ClientHello without a host name.
    Missing,
    /// More bytes are needed.
    Incomplete,
    /// Not a TLS handshake, or a malformed one.
    NotTls,
}

/// The server name of the ClientHello at the start of `bytes`.
pub fn server_name(bytes: &[u8]) -> ServerName {
    let handshake = match handshake_bytes(bytes) {
        Ok(handshake) => handshake,
        Err(name) => return name,
    };
    // Handshake header: type, then a 24-bit length.
    let Some(&[kind, high, middle, low]) = handshake.get(..4) else {
        return ServerName::Incomplete;
    };
    if kind != CLIENT_HELLO {
        return ServerName::NotTls;
    }
    let length = usize::from(high) << 16 | usize::from(middle) << 8 | usize::from(low);
    if length > MAX_HELLO_BYTES {
        return ServerName::NotTls;
    }
    let Some(body) = handshake.get(4..4 + length) else {
        return ServerName::Incomplete;
    };
    parse_client_hello(body).unwrap_or(ServerName::NotTls)
}

/// The handshake bytes carried by the handshake records at the start of `bytes`, joined.
fn handshake_bytes(bytes: &[u8]) -> Result<Vec<u8>, ServerName> {
    let mut handshake = Vec::new();
    let mut rest = bytes;
    loop {
        let Some(header) = rest.get(..RECORD_HEADER_BYTES) else {
            return if handshake.is_empty() && !rest.is_empty() && rest[0] != HANDSHAKE_RECORD {
                Err(ServerName::NotTls)
            } else {
                Ok(handshake)
            };
        };
        if header[0] != HANDSHAKE_RECORD || header[1] != 3 {
            return if handshake.is_empty() {
                Err(ServerName::NotTls)
            } else {
                Ok(handshake)
            };
        }
        let length = usize::from(u16::from_be_bytes([header[3], header[4]]));
        let Some(fragment) = rest.get(RECORD_HEADER_BYTES..RECORD_HEADER_BYTES + length) else {
            return Ok(handshake);
        };
        handshake.extend_from_slice(fragment);
        rest = &rest[RECORD_HEADER_BYTES + length..];
    }
}

/// A cursor over a ClientHello body; None at its end means malformed.
struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        let taken = self.bytes.get(..count)?;
        self.bytes = &self.bytes[count..];
        Some(taken)
    }

    fn byte(&mut self) -> Option<u8> {
        self.take(1).map(|taken| taken[0])
    }

    fn u16(&mut self) -> Option<u16> {
        self.take(2)
            .map(|taken| u16::from_be_bytes([taken[0], taken[1]]))
    }

    /// A vector with a one-byte length.
    fn short_vector(&mut self) -> Option<&'a [u8]> {
        let length = self.byte()?;
        self.take(usize::from(length))
    }

    /// A vector with a two-byte length.
    fn vector(&mut self) -> Option<&'a [u8]> {
        let length = self.u16()?;
        self.take(usize::from(length))
    }
}

fn parse_client_hello(body: &[u8]) -> Option<ServerName> {
    let mut reader = Reader { bytes: body };
    // Legacy version and random.
    reader.take(2 + 32)?;
    reader.short_vector()?; // legacy session id
    reader.vector()?; // cipher suites
    reader.short_vector()?; // legacy compression methods
    if reader.bytes.is_empty() {
        return Some(ServerName::Missing);
    }
    let mut extensions = Reader {
        bytes: reader.vector()?,
    };
    while !extensions.bytes.is_empty() {
        let kind = extensions.u16()?;
        let data = extensions.vector()?;
        if kind == SERVER_NAME_EXTENSION {
            return host_name(data);
        }
    }
    Some(ServerName::Missing)
}

fn host_name(extension: &[u8]) -> Option<ServerName> {
    let mut list = Reader {
        bytes: Reader { bytes: extension }.vector()?,
    };
    while !list.bytes.is_empty() {
        let kind = list.byte()?;
        let name = list.vector()?;
        if kind == HOST_NAME_TYPE {
            let name = std::str::from_utf8(name).ok()?;
            // A name that is no DNS name stays as sent, so the mismatch names it.
            let name =
                cockpit_sandbox::normalized_name(name).unwrap_or_else(|| name.to_ascii_lowercase());
            return Some(ServerName::Named(name));
        }
    }
    Some(ServerName::Missing)
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A ClientHello naming `name`, or none, in records of at most `record_size` bytes.
    pub fn client_hello(name: Option<&str>, record_size: usize) -> Vec<u8> {
        let mut extensions = Vec::new();
        // An extension before the name: supported versions, TLS 1.3.
        extensions.extend_from_slice(&[0, 43, 0, 3, 2, 3, 4]);
        if let Some(name) = name {
            let name = name.as_bytes();
            let entry_length = u16::try_from(name.len() + 3).unwrap();
            let data_length = entry_length + 2;
            extensions.extend_from_slice(&[0, 0]);
            extensions.extend_from_slice(&data_length.to_be_bytes());
            extensions.extend_from_slice(&entry_length.to_be_bytes());
            extensions.push(HOST_NAME_TYPE);
            extensions.extend_from_slice(&u16::try_from(name.len()).unwrap().to_be_bytes());
            extensions.extend_from_slice(name);
        }
        let mut body = vec![3, 3];
        body.extend_from_slice(&[7; 32]);
        body.push(0);
        body.extend_from_slice(&[0, 2, 0x13, 0x01]);
        body.extend_from_slice(&[1, 0]);
        body.extend_from_slice(&u16::try_from(extensions.len()).unwrap().to_be_bytes());
        body.extend_from_slice(&extensions);
        let length = u32::try_from(body.len()).unwrap().to_be_bytes();
        let mut handshake = vec![CLIENT_HELLO, length[1], length[2], length[3]];
        handshake.extend_from_slice(&body);
        let mut records = Vec::new();
        for fragment in handshake.chunks(record_size) {
            records.extend_from_slice(&[HANDSHAKE_RECORD, 3, 1]);
            records.extend_from_slice(&u16::try_from(fragment.len()).unwrap().to_be_bytes());
            records.extend_from_slice(fragment);
        }
        records
    }

    #[test]
    fn a_client_hello_gives_its_server_name_in_lower_case() {
        let hello = client_hello(Some("API.Example.com."), 16384);

        assert_eq!(
            server_name(&hello),
            ServerName::Named("api.example.com".to_owned())
        );
    }

    #[test]
    fn a_client_hello_split_across_records_gives_its_server_name() {
        let hello = client_hello(Some("api.example.com"), 20);

        assert_eq!(
            server_name(&hello),
            ServerName::Named("api.example.com".to_owned())
        );
    }

    #[test]
    fn a_client_hello_without_a_server_name_is_missing_one() {
        assert_eq!(server_name(&client_hello(None, 16384)), ServerName::Missing);
    }

    #[test]
    fn each_shorter_prefix_of_a_client_hello_needs_more_bytes() {
        let hello = client_hello(Some("api.example.com"), 16384);

        for end in 0..hello.len() {
            assert_eq!(server_name(&hello[..end]), ServerName::Incomplete, "{end}");
        }
    }

    #[test]
    fn bytes_that_are_not_a_tls_handshake_are_not_tls() {
        assert_eq!(server_name(b"GET / HTTP/1.1\r\n\r\n"), ServerName::NotTls);
    }
}
