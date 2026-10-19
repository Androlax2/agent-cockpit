//! Web Push for the phone page (claude-remote): a notification on the phone when a session needs
//! Theo or an ask-to-run waits. The payload is encrypted for the phone's browser alone (RFC 8291,
//! aes128gcm), so the push service (Apple's, Google's) carries it without reading it; the server
//! signs each request with its own key (VAPID, RFC 8292), kept in the claude-remote state folder.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair};
use ring::{aead, agreement, hkdf};
use serde_json::{Value, json};
use std::time::Duration;

/// Records of at most 4096 bytes: one record holds any notification this sends.
const RECORD_SIZE: u32 = 4096;
const VAPID_VALID_S: i64 = 12 * 3600;
/// The push services of Safari, Chrome and Firefox: a subscription elsewhere would make the
/// computer post wherever the page said.
const PUSH_HOSTS: [&str; 3] = ["web.push.apple.com", "fcm.googleapis.com", "updates.push.services.mozilla.com"];
const PUSH_HOST_SUFFIXES: [&str; 2] = [".push.apple.com", ".notify.windows.com"];

fn base64url(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

fn from_base64url(text: &str) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD.decode(text.trim_end_matches('=')).ok()
}

fn endpoint_host(endpoint: &str) -> Option<&str> {
    let rest = endpoint.strip_prefix("https://")?;
    let authority = rest.split('/').next()?;
    (!authority.contains('@')).then(|| authority.split(':').next().unwrap_or(authority))
}

/// A subscription the page sent, checked: an https endpoint of a known push service, a P-256
/// public key and a 16-byte secret. What is kept is only these three.
pub fn checked_subscription(subscription: &Value) -> Result<Value, String> {
    let endpoint = subscription.get("endpoint").and_then(Value::as_str).ok_or("no endpoint")?;
    let host = endpoint_host(endpoint).ok_or("the endpoint is not an https URL")?;
    let is_known = PUSH_HOSTS.contains(&host) || PUSH_HOST_SUFFIXES.iter().any(|suffix| host.ends_with(suffix));
    if !is_known || endpoint.len() > 1024 {
        return Err(format!("{host} is not a known push service"));
    }
    let key = |name: &str, length: usize| {
        subscription.get("keys").and_then(|keys| keys.get(name)).and_then(Value::as_str).and_then(from_base64url)
            .filter(|bytes| bytes.len() == length).ok_or(format!("the subscription's {name} is not {length} bytes"))
    };
    Ok(json!({"endpoint": endpoint, "p256dh": base64url(&key("p256dh", 65)?), "auth": base64url(&key("auth", 16)?)}))
}

/// The server's signing key, made the first time and kept (PKCS#8) in `vapid.json`.
fn vapid_key() -> Result<EcdsaKeyPair, String> {
    let random = SystemRandom::new();
    let stored = crate::remote::read_private("vapid.json");
    let pkcs8 = match stored.get("pkcs8").and_then(Value::as_str).and_then(from_base64url) {
        Some(pkcs8) => pkcs8,
        None => {
            let made = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &random).map_err(|_| "cannot make a signing key")?;
            crate::remote::write_private("vapid.json", &json!({"pkcs8": base64url(made.as_ref())})).map_err(|error| error.to_string())?;
            made.as_ref().to_vec()
        }
    };
    EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &pkcs8, &random).map_err(|error| format!("vapid.json: {error}"))
}

/// The public half of the signing key, for the page's subscription (applicationServerKey).
pub fn public_key() -> Result<String, String> {
    Ok(base64url(vapid_key()?.public_key().as_ref()))
}

/// The Authorization header of a push request to `endpoint`: a JWT signed with ES256 (RFC 8292).
fn vapid_authorization(endpoint: &str, contact: &str) -> Result<String, String> {
    signed_authorization(&vapid_key()?, endpoint, contact)
}

fn signed_authorization(key: &EcdsaKeyPair, endpoint: &str, contact: &str) -> Result<String, String> {
    let audience = format!("https://{}", endpoint_host(endpoint).ok_or("not an https endpoint")?);
    let header = base64url(br#"{"typ":"JWT","alg":"ES256"}"#);
    let claims = base64url(json!({"aud": audience, "exp": crate::util::now_secs() + VAPID_VALID_S, "sub": contact}).to_string().as_bytes());
    let signed = format!("{header}.{claims}");
    let signature = key.sign(&SystemRandom::new(), signed.as_bytes()).map_err(|_| "cannot sign")?;
    Ok(format!("vapid t={signed}.{}, k={}", base64url(signature.as_ref()), base64url(key.public_key().as_ref())))
}

struct Length(usize);

impl hkdf::KeyType for Length {
    fn len(&self) -> usize {
        self.0
    }
}

fn hkdf(salt: &[u8], secret: &[u8], info: &[u8], output: &mut [u8]) {
    let key = hkdf::Salt::new(hkdf::HKDF_SHA256, salt).extract(secret);
    key.expand(&[info], Length(output.len())).and_then(|okm| okm.fill(output)).expect("HKDF lengths within limits");
}

/// The content key and nonce (RFC 8291 §3.3, 3.4), from the shared ECDH secret.
pub fn derive_keys(ecdh_secret: &[u8], auth_secret: &[u8], ua_public: &[u8], as_public: &[u8], salt: &[u8]) -> ([u8; 16], [u8; 12]) {
    let key_info = [b"WebPush: info\0".as_slice(), ua_public, as_public].concat();
    let mut ikm = [0u8; 32];
    hkdf(auth_secret, ecdh_secret, &key_info, &mut ikm);
    let (mut cek, mut nonce) = ([0u8; 16], [0u8; 12]);
    hkdf(salt, &ikm, b"Content-Encoding: aes128gcm\0", &mut cek);
    hkdf(salt, &ikm, b"Content-Encoding: nonce\0", &mut nonce);
    (cek, nonce)
}

/// The aes128gcm body (RFC 8188): salt, record size, the server's ephemeral key, one record.
pub fn seal(cek: &[u8; 16], nonce: &[u8; 12], salt: &[u8], as_public: &[u8], plaintext: &[u8]) -> Vec<u8> {
    let key = aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_128_GCM, cek).expect("a 16-byte key"));
    let mut record = [plaintext, &[2u8]].concat(); // 2: the last record's padding delimiter
    key.seal_in_place_append_tag(aead::Nonce::assume_unique_for_key(*nonce), aead::Aad::empty(), &mut record).expect("a record within limits");
    [salt, &RECORD_SIZE.to_be_bytes(), &[as_public.len() as u8], as_public, &record].concat()
}

/// `plaintext` encrypted for one subscription (its p256dh and auth), with a fresh key and salt.
pub fn encrypt(subscription: &Value, plaintext: &[u8]) -> Result<Vec<u8>, String> {
    let field = |name: &str| subscription.get(name).and_then(Value::as_str).and_then(from_base64url).ok_or(format!("no {name}"));
    let (ua_public, auth_secret) = (field("p256dh")?, field("auth")?);
    let random = SystemRandom::new();
    let mut salt = [0u8; 16];
    random.fill(&mut salt).map_err(|_| "no randomness")?;
    let ephemeral = agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &random).map_err(|_| "cannot make a key")?;
    let as_public = ephemeral.compute_public_key().map_err(|_| "cannot make a key")?.as_ref().to_vec();
    let peer = agreement::UnparsedPublicKey::new(&agreement::ECDH_P256, &ua_public);
    let ecdh_secret = agreement::agree_ephemeral(ephemeral, &peer, |secret| secret.to_vec()).map_err(|_| "the phone's key is not on P-256")?;
    let (cek, nonce) = derive_keys(&ecdh_secret, &auth_secret, &ua_public, &as_public, &salt);
    Ok(seal(&cek, &nonce, &salt, &as_public, plaintext))
}

/// How a push went.
#[derive(Debug, PartialEq)]
pub enum Delivery {
    Sent,
    /// The push service no longer knows the subscription (404, 410): the phone unsubscribed.
    Gone,
    Failed(String),
}

/// One notification to one subscription. `topic` replaces an earlier one of the same topic still
/// waiting; `contact` is the VAPID subject (the page's own address).
pub fn send(subscription: &Value, notification: &Value, topic: &str, contact: &str) -> Delivery {
    let Some(endpoint) = subscription.get("endpoint").and_then(Value::as_str) else { return Delivery::Failed("no endpoint".into()) };
    let prepared = encrypt(subscription, notification.to_string().as_bytes()).and_then(|body| Ok((body, vapid_authorization(endpoint, contact)?)));
    let (body, authorization) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => return Delivery::Failed(error),
    };
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(10))).build().into();
    let topic: String = topic.chars().filter(char::is_ascii_alphanumeric).take(32).collect();
    let sent = agent.post(endpoint).header("TTL", "3600").header("Urgency", "high").header("Topic", &topic)
        .header("Content-Encoding", "aes128gcm").header("Content-Type", "application/octet-stream")
        .header("Authorization", &authorization).send(&body[..]);
    match sent {
        Ok(_) => Delivery::Sent,
        Err(ureq::Error::StatusCode(404 | 410)) => Delivery::Gone,
        Err(error) => Delivery::Failed(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(text: &str) -> Vec<u8> {
        from_base64url(text).unwrap()
    }

    // RFC 8291, Appendix A (checked against Python's cryptography before it was written here).
    const UA_PUBLIC: &str = "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4";
    const AS_PUBLIC: &str = "BP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8";
    const ECDH: &str = "kyrL1jIIOHEzg3sM2ZWRHDRB62YACZhhSlknJ672kSs";
    const AUTH: &str = "BTBZMqHH6r4Tts7J_aSIgg";
    const SALT: &str = "DGv6ra1nlYgDCS1FRnbzlw";

    #[test]
    fn the_keys_and_the_body_match_rfc_8291s_example() {
        let (cek, nonce) = derive_keys(&bytes(ECDH), &bytes(AUTH), &bytes(UA_PUBLIC), &bytes(AS_PUBLIC), &bytes(SALT));
        assert_eq!((base64url(&cek), base64url(&nonce)), ("oIhVW04MRdy2XN9CiKLxTg".to_string(), "4h_95klXJ5E_qnoN".to_string()));
        let body = seal(&cek, &nonce, &bytes(SALT), &bytes(AS_PUBLIC), &bytes("V2hlbiBJIGdyb3cgdXAsIEkgd2FudCB0byBiZSBhIHdhdGVybWVsb24"));
        assert_eq!(base64url(&body), "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPTpK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN");
    }

    #[test]
    fn the_phone_decrypts_what_encrypt_sends_it() {
        let random = SystemRandom::new();
        let phone_key = agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &random).unwrap();
        let phone_public = phone_key.compute_public_key().unwrap().as_ref().to_vec();
        let subscription = json!({"endpoint": "https://web.push.apple.com/x", "p256dh": base64url(&phone_public), "auth": AUTH});
        let body = encrypt(&subscription, b"proj-a needs you").unwrap();
        let (salt, rest) = body.split_at(16);
        assert_eq!(rest[..4], RECORD_SIZE.to_be_bytes());
        let (as_public, record) = rest[5..].split_at(rest[4] as usize);
        let peer = agreement::UnparsedPublicKey::new(&agreement::ECDH_P256, as_public);
        let ecdh_secret = agreement::agree_ephemeral(phone_key, &peer, |secret| secret.to_vec()).unwrap();
        let (cek, nonce) = derive_keys(&ecdh_secret, &bytes(AUTH), &phone_public, as_public, salt);
        let key = aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_128_GCM, &cek).unwrap());
        let mut record = record.to_vec();
        let opened = key.open_in_place(aead::Nonce::assume_unique_for_key(nonce), aead::Aad::empty(), &mut record).unwrap();
        assert_eq!(opened, b"proj-a needs you\x02");
    }

    #[test]
    fn the_vapid_header_carries_a_valid_es256_signature_for_the_push_services_origin() {
        let random = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &random).unwrap();
        let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &random).unwrap();
        let header = signed_authorization(&key, "https://web.push.apple.com/QGuQyavXutnMH", "https://box.ts.net").unwrap();
        let (token, public) = header.strip_prefix("vapid t=").unwrap().split_once(", k=").unwrap();
        let (signed, signature) = token.rsplit_once('.').unwrap();
        ring::signature::UnparsedPublicKey::new(&ring::signature::ECDSA_P256_SHA256_FIXED, bytes(public))
            .verify(signed.as_bytes(), &bytes(signature)).expect("the signature does not verify");
        let claims: Value = serde_json::from_slice(&bytes(signed.split('.').nth(1).unwrap())).unwrap();
        assert_eq!((claims["aud"].as_str(), claims["sub"].as_str()), (Some("https://web.push.apple.com"), Some("https://box.ts.net")));
    }

    #[test]
    fn only_a_known_push_services_https_endpoint_with_right_sized_keys_is_kept() {
        let keys = json!({"p256dh": UA_PUBLIC, "auth": AUTH});
        for (endpoint, keys, is_kept) in [
            ("https://web.push.apple.com/abc", keys.clone(), true),
            ("https://fcm.googleapis.com/fcm/send/abc", keys.clone(), true),
            ("http://web.push.apple.com/abc", keys.clone(), false),
            ("https://192.168.1.1/admin", keys.clone(), false),
            ("https://web.push.apple.com.evil.example/abc", keys.clone(), false),
            ("https://user@web.push.apple.com/abc", keys.clone(), false),
            ("https://web.push.apple.com/abc", json!({"p256dh": AUTH, "auth": AUTH}), false),
            ("https://web.push.apple.com/abc", json!({"p256dh": UA_PUBLIC}), false),
        ] {
            let checked = checked_subscription(&json!({"endpoint": endpoint, "keys": keys}));
            assert_eq!(checked.is_ok(), is_kept, "{endpoint} {keys}: {checked:?}");
        }
    }
}
