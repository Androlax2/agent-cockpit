//! A secret the owner typed for a run, recognised in that run's output without being kept
//! (#1386). When the owner types a hidden line or answers sudo's ask, the run draws a random
//! key and keeps only `HMAC-SHA-256(key, form)` and the length of each form the secret may be
//! printed in: the raw bytes, base64 at each of its three alignments, lower- and upper-case
//! hex, and URL encoding. A window of each form's length slides over the output; a match
//! becomes `[redacted]` and is counted. So that each byte does not cost an HMAC per length,
//! a rolling hash under a random base screens the windows first: only one whose top 16 bits
//! equal a form's is checked against its HMAC. The secret itself and every form are wiped once
//! fingerprinted. The key and fingerprints live as long as the run's output does.
//!
//! This is a safety net for accidents (a script that echoes its input, a tool that logs
//! it): a command built to leak the secret can transform it first.

use sha2::{Digest, Sha256};

use crate::redact::REDACTED;

/// Shorter typed lines (a `y`, an empty answer) are not fingerprinted: their bytes are too
/// common in ordinary output to redact.
pub(crate) const MIN_TYPED_SECRET_BYTES: usize = 4;
/// An encoded form shorter than this (the base64 of a 4-byte secret) is not fingerprinted:
/// a few base64 or hex characters turn up by chance in hashes and tokens. The raw form is
/// kept from [`MIN_TYPED_SECRET_BYTES`].
const MIN_ENCODED_FORM_BYTES: usize = 6;
const BLOCK: usize = 64;
const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Overwrites `bytes` with zeros. `black_box` keeps the compiler from dropping the writes
/// to memory about to be freed; the workspace denies `unsafe`, so no volatile write.
pub fn wipe(bytes: &mut [u8]) {
    bytes.fill(0);
    std::hint::black_box(bytes);
}

/// [`wipe`] for a whole vector's allocation, spare capacity included, then empties it.
pub fn wipe_vec(bytes: &mut Vec<u8>) {
    bytes.fill(0);
    bytes.resize(bytes.capacity(), 0);
    std::hint::black_box(&mut *bytes);
    bytes.clear();
}

/// The fingerprints of every secret typed for one run, and the output bytes held back
/// because a form may continue in the next read.
#[derive(Default)]
pub(crate) struct TypedSecrets {
    keyed: Option<Keyed>,
    /// Distinct form lengths, longest first.
    lengths: Vec<usize>,
    fingerprints: Vec<Fingerprint>,
    held: Vec<u8>,
    redacted: u64,
}

impl std::fmt::Debug for TypedSecrets {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TypedSecrets")
            .field("forms", &self.fingerprints.len())
            .field("held", &self.held.len())
            .field("redacted", &self.redacted)
            .finish_non_exhaustive()
    }
}

impl Drop for TypedSecrets {
    fn drop(&mut self) {
        wipe_vec(&mut self.held);
        for fingerprint in &mut self.fingerprints {
            fingerprint.tag = 0;
            wipe(&mut fingerprint.mac);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Fingerprint {
    length: usize,
    /// The top 16 bits of the form's rolling hash: the screen.
    tag: u16,
    mac: [u8; 32],
}

/// The run's HMAC-SHA-256 key and the rolling hash's base, wiped when dropped. The key is
/// kept as bytes rather than as hash states absorbed from it, which could not be wiped: an
/// HMAC is computed only for a window the screen passed, so the extra block costs little.
struct Keyed {
    key: [u8; BLOCK],
    base: u64,
}

impl Drop for Keyed {
    fn drop(&mut self) {
        wipe(&mut self.key);
        self.base = 0;
        std::hint::black_box(&mut self.base);
    }
}

impl Keyed {
    fn new(key: [u8; BLOCK], base: u64) -> Keyed {
        Keyed {
            key,
            base: base | 1,
        }
    }

    fn mac(&self, message: &[u8]) -> [u8; 32] {
        let mut pad = self.key.map(|byte| byte ^ 0x36);
        let inner = Sha256::new()
            .chain_update(pad)
            .chain_update(message)
            .finalize();
        pad = self.key.map(|byte| byte ^ 0x5c);
        let mac = Sha256::new()
            .chain_update(pad)
            .chain_update(inner)
            .finalize();
        wipe(&mut pad);
        mac.into()
    }

    fn fingerprint(&self, form: &[u8]) -> Fingerprint {
        Fingerprint {
            length: form.len(),
            tag: tag(form.iter().fold(0, |hash, &byte| self.roll_in(hash, byte))),
            mac: self.mac(form),
        }
    }

    fn roll_in(&self, hash: u64, byte: u8) -> u64 {
        hash.wrapping_mul(self.base).wrapping_add(u64::from(byte))
    }

    /// For each start in `0..until` where a window of `length` fits in `window`, whether its
    /// rolling hash carries one of `tags`.
    fn screened(&self, window: &[u8], length: usize, until: usize, tags: &[u16]) -> Vec<bool> {
        let starts = until.min((window.len() + 1).saturating_sub(length));
        let mut passed = vec![false; starts];
        if starts == 0 {
            return passed;
        }
        let leaving = (1..length).fold(1u64, |power, _| power.wrapping_mul(self.base));
        let mut hash = window[..length]
            .iter()
            .fold(0, |hash, &byte| self.roll_in(hash, byte));
        for start in 0..starts {
            if start > 0 {
                let left = u64::from(window[start - 1]).wrapping_mul(leaving);
                hash = self.roll_in(hash.wrapping_sub(left), window[start + length - 1]);
            }
            passed[start] = tags.contains(&tag(hash));
        }
        passed
    }
}

fn tag(hash: u64) -> u16 {
    (hash >> 48) as u16
}

impl TypedSecrets {
    /// Fingerprints `secret` for this run, drawing the run's key at the first one, then wipes
    /// `secret`. A secret shorter than [`MIN_TYPED_SECRET_BYTES`] is wiped unfingerprinted.
    pub(crate) fn add(&mut self, secret: &mut [u8]) -> std::io::Result<()> {
        if secret.len() < MIN_TYPED_SECRET_BYTES {
            wipe(secret);
            return Ok(());
        }
        if self.keyed.is_none() {
            let (mut key, base) = random_key()?;
            self.keyed = Some(Keyed::new(key, base));
            wipe(&mut key);
        }
        let keyed = self.keyed.as_ref().expect("the key was drawn above");
        for (index, mut form) in forms(secret).into_iter().enumerate() {
            let is_raw = index == 0;
            if is_raw || form.len() >= MIN_ENCODED_FORM_BYTES {
                let fingerprint = keyed.fingerprint(&form);
                if !self.fingerprints.contains(&fingerprint) {
                    self.fingerprints.push(fingerprint);
                }
            }
            wipe_vec(&mut form);
        }
        wipe(secret);
        self.lengths = self.fingerprints.iter().map(|form| form.length).collect();
        self.lengths.sort_unstable_by(|left, right| right.cmp(left));
        self.lengths.dedup();
        Ok(())
    }

    /// How many times a typed secret was redacted so far.
    pub(crate) fn redacted(&self) -> u64 {
        self.redacted
    }

    /// Appends `chunk` to `output`, each typed secret in it redacted. The last bytes that may
    /// begin a form continuing in the next chunk are held back until it comes or
    /// [`TypedSecrets::release`] is called.
    pub(crate) fn scrub_into(&mut self, chunk: &[u8], output: &mut Vec<u8>) {
        let Some(&longest) = self.lengths.first() else {
            output.extend_from_slice(chunk);
            return;
        };
        let mut window = std::mem::take(&mut self.held);
        window.extend_from_slice(chunk);
        let held_from = window.len().saturating_sub(longest - 1);
        let rest = self.scrub_until(&window, held_from, output);
        self.held = window[rest..].to_vec();
        wipe_vec(&mut window);
    }

    /// Appends the bytes held back to `output`, each typed secret in them redacted: at the
    /// end of the output, or when it went quiet and what it shows last must be seen.
    pub(crate) fn release(&mut self, output: &mut Vec<u8>) {
        if self.held.is_empty() {
            return;
        }
        let mut window = std::mem::take(&mut self.held);
        self.scrub_until(&window, window.len(), output);
        wipe_vec(&mut window);
    }

    /// Scrubs `window` into `output` up to the first position at or after `until` where no
    /// match started; answers that position.
    fn scrub_until(&mut self, window: &[u8], until: usize, output: &mut Vec<u8>) -> usize {
        let Some(keyed) = &self.keyed else {
            output.extend_from_slice(&window[..until]);
            return until;
        };
        let screens: Vec<(usize, Vec<bool>)> = self
            .lengths
            .iter()
            .map(|&length| {
                let tags: Vec<u16> = self
                    .fingerprints
                    .iter()
                    .filter(|form| form.length == length)
                    .map(|form| form.tag)
                    .collect();
                (length, keyed.screened(window, length, until, &tags))
            })
            .collect();
        let mut at = 0;
        let mut unmatched_from = 0;
        while at < until {
            let found = screens.iter().find_map(|(length, passed)| {
                let candidate = window.get(at..at + length)?;
                if !passed.get(at).copied().unwrap_or(false) {
                    return None;
                }
                let fingerprint = keyed.fingerprint(candidate);
                self.fingerprints.contains(&fingerprint).then_some(*length)
            });
            match found {
                Some(length) => {
                    output.extend_from_slice(&window[unmatched_from..at]);
                    output.extend_from_slice(REDACTED.as_bytes());
                    self.redacted += 1;
                    at += length;
                    unmatched_from = at;
                }
                None => at += 1,
            }
        }
        output.extend_from_slice(&window[unmatched_from..at]);
        at
    }
}

/// The run's HMAC key, 64 random bytes, and its rolling hash's random base.
fn random_key() -> std::io::Result<([u8; BLOCK], u64)> {
    use std::io::Read;
    let mut random = std::fs::File::open("/dev/urandom")?;
    let mut key = [0; BLOCK];
    random.read_exact(&mut key)?;
    let mut base = [0; 8];
    random.read_exact(&mut base)?;
    Ok((key, u64::from_le_bytes(base)))
}

/// Every form `secret` may be printed in, the raw bytes first. Each is wiped by the caller.
fn forms(secret: &[u8]) -> Vec<Vec<u8>> {
    let mut forms = vec![secret.to_vec()];
    forms.extend((0..3).map(|alignment| base64_inside(secret, alignment)));
    forms.push(hex(secret, b"0123456789abcdef"));
    forms.push(hex(secret, b"0123456789ABCDEF"));
    forms.push(url_encoded(secret));
    forms
}

/// The base64 characters that hold only `secret`'s bits when it follows `alignment` bytes
/// (0 to 2) of other data in the encoded stream: the same whatever those bytes are and
/// whatever follows.
fn base64_inside(secret: &[u8], alignment: usize) -> Vec<u8> {
    let start_bit = 8 * alignment;
    let end_bit = start_bit + 8 * secret.len();
    let first = start_bit.div_ceil(6);
    let last = end_bit / 6;
    (first..last)
        .map(|index| {
            let mut sextet = 0usize;
            for bit in 6 * index..6 * index + 6 {
                let byte = secret[bit / 8 - alignment];
                sextet = (sextet << 1) | usize::from((byte >> (7 - bit % 8)) & 1);
            }
            BASE64[sextet]
        })
        .collect()
}

fn hex(secret: &[u8], digits: &[u8; 16]) -> Vec<u8> {
    secret
        .iter()
        .flat_map(|byte| {
            [
                digits[usize::from(byte >> 4)],
                digits[usize::from(byte & 0xf)],
            ]
        })
        .collect()
}

/// RFC 3986 percent-encoding: unreserved characters stay, every other byte is `%XX`.
fn url_encoded(secret: &[u8]) -> Vec<u8> {
    let digits = b"0123456789ABCDEF";
    let mut encoded = Vec::with_capacity(secret.len() * 3);
    for &byte in secret {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            encoded.push(byte);
        } else {
            encoded.extend([
                b'%',
                digits[usize::from(byte >> 4)],
                digits[usize::from(byte & 0xf)],
            ]);
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &[u8] = b"hunter 22/xyz!";

    fn typed(secret: &[u8]) -> TypedSecrets {
        let mut secrets = TypedSecrets::default();
        secrets.add(&mut secret.to_vec()).unwrap();
        secrets
    }

    /// `output` read in chunks of `chunk` bytes, then the held bytes released.
    fn scrubbed(secrets: &mut TypedSecrets, output: &[u8], chunk: usize) -> Vec<u8> {
        let mut scrubbed = Vec::new();
        for piece in output.chunks(chunk) {
            secrets.scrub_into(piece, &mut scrubbed);
        }
        secrets.release(&mut scrubbed);
        scrubbed
    }

    #[test]
    fn the_base64_inside_matches_the_encoders_output_at_each_alignment() {
        // `printf 'x%s' 'hunter 22/xyz!' | base64` and its siblings.
        let encoded = [
            "aHVudGVyIDIyL3h5eiE=",
            "eGh1bnRlciAyMi94eXoh",
            "eHhodW50ZXIgMjIveHl6IQ==",
        ];
        for (alignment, encoded) in encoded.into_iter().enumerate() {
            let inside = String::from_utf8(base64_inside(SECRET, alignment)).unwrap();
            assert!(
                encoded.contains(&inside),
                "{alignment}: {inside} in {encoded}"
            );
            assert_eq!(inside.len(), 18, "{alignment}");
        }
    }

    #[test]
    fn hex_and_url_forms_match_the_usual_tools() {
        assert_eq!(
            hex(SECRET, b"0123456789abcdef"),
            b"68756e7465722032322f78797a21"
        );
        assert_eq!(
            hex(SECRET, b"0123456789ABCDEF"),
            b"68756E7465722032322F78797A21"
        );
        assert_eq!(url_encoded(SECRET), b"hunter%2022%2Fxyz%21");
    }

    #[test]
    fn a_secret_split_at_every_chunk_size_is_redacted_once() {
        let output = b"before hunter 22/xyz! after";
        for chunk in 1..=output.len() {
            let mut secrets = typed(SECRET);

            let scrubbed = scrubbed(&mut secrets, output, chunk);

            assert_eq!(scrubbed, b"before [redacted] after", "chunks of {chunk}");
            assert_eq!(secrets.redacted(), 1, "chunks of {chunk}");
        }
    }

    #[test]
    fn output_without_the_secret_passes_unchanged_in_any_chunking() {
        let output: Vec<u8> = (0..=255u8).cycle().take(3000).collect();
        for chunk in [1, 7, 13, 64, 1000, 3000] {
            let mut secrets = typed(SECRET);

            assert_eq!(scrubbed(&mut secrets, &output, chunk), output, "{chunk}");
            assert_eq!(secrets.redacted(), 0);
        }
    }

    #[test]
    fn output_passes_unchanged_and_unheld_before_any_secret_is_typed() {
        let mut secrets = TypedSecrets::default();
        let mut scrubbed = Vec::new();

        secrets.scrub_into(b"hunter 22/xyz!", &mut scrubbed);

        assert_eq!(scrubbed, b"hunter 22/xyz!");
    }

    #[test]
    fn each_of_several_typed_secrets_is_redacted() {
        let mut secrets = typed(SECRET);
        secrets.add(&mut b"second-one".to_vec()).unwrap();

        let scrubbed = scrubbed(&mut secrets, b"hunter 22/xyz! and second-one", 5);

        assert_eq!(scrubbed, b"[redacted] and [redacted]");
        assert_eq!(secrets.redacted(), 2);
    }

    #[test]
    fn a_line_shorter_than_the_minimum_is_not_fingerprinted() {
        let mut secrets = typed(b"yes");

        assert_eq!(scrubbed(&mut secrets, b"yes, yes", 3), b"yes, yes");
    }

    #[test]
    fn a_short_secrets_short_base64_forms_are_not_fingerprinted() {
        // `abcd` in base64, at alignment 0, is `YWJj` and then `ZA==`: inside, `YWJjZ`.
        let mut secrets = typed(b"abcd");

        assert_eq!(
            scrubbed(&mut secrets, b"YWJjZA== abcd 61626364", 4),
            b"YWJjZA== [redacted] [redacted]"
        );
    }

    #[test]
    fn adding_wipes_the_secret_it_was_handed() {
        let mut secret = SECRET.to_vec();

        TypedSecrets::default().add(&mut secret).unwrap();

        assert_eq!(secret, vec![0; SECRET.len()]);
    }

    #[test]
    fn the_hmac_matches_rfc_4231_test_case_2() {
        let mut key = [0; BLOCK];
        key[..4].copy_from_slice(b"Jefe");

        let mac = Keyed::new(key, 1).mac(b"what do ya want for nothing?");

        assert_eq!(
            hex(&mac, b"0123456789abcdef"),
            b"5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }
}
