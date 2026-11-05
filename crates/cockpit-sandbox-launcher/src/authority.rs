//! The per-session CA (#832, #546 D1 stage C): generated in the launcher's memory at each
//! launch, used once to issue a certificate for each host that carries a bound credential,
//! then dropped. Its key is never written anywhere; only its certificate leaves the launcher,
//! in the trust files the sandbox reads. The CA's name constraints permit those hosts alone,
//! so its certificate vouches for nothing else even if a client trusted it everywhere.
//!
//! Upstream servers are verified against the host's CA bundle, the same roots the session's
//! own clients verify every tunneled host against.

use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use cockpit_sandbox::Interception;
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose,
    GeneralSubtree, IsCa, Issuer, KeyPair, KeyUsagePurpose, NameConstraints,
};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

use crate::intercept::Credential;

/// How long the CA and its certificates are valid from the day before the launch: clocks
/// inside and outside agree, so a day absorbs any skew; a session outliving it relaunches.
const VALID_DAYS: i64 = 366;

/// The one application protocol the proxy speaks inside an intercepted tunnel, to both sides.
const HTTP_1_1: &[u8] = b"http/1.1";

/// The interception a launch serves: each bound host's credential and server configuration,
/// and the client configuration that verifies upstream servers.
pub struct Authority {
    hosts: BTreeMap<String, BoundHost>,
    upstream: Arc<rustls::ClientConfig>,
}

/// One host that carries a credential.
pub struct BoundHost {
    pub credential: Credential,
    /// Presents the CA's certificate for this host to the sandbox's client.
    pub server: Arc<rustls::ServerConfig>,
}

impl Authority {
    /// Generates the CA for `session`, issues each bound host's certificate, and writes the
    /// trust files `interception` names, before the program starts. `values` are the bound
    /// credentials' values, in the order of `interception`'s credentials.
    pub fn create(
        session: &str,
        interception: &Interception,
        values: Vec<String>,
    ) -> Result<Authority, String> {
        if values.len() != interception.credentials.len() {
            return Err(format!(
                "the credentials file holds {} values for {} bound credentials",
                values.len(),
                interception.credentials.len()
            ));
        }
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let system_bundle = std::fs::read(&interception.system_bundle).map_err(|error| {
            format!(
                "cannot read the host's CA bundle {}: {error}",
                interception.system_bundle.display()
            )
        })?;
        let upstream = upstream_config(&provider, &system_bundle, &interception.system_bundle)?;
        let host_names: Vec<&str> = interception
            .credentials
            .iter()
            .flat_map(|credential| credential.hosts.iter().map(|host| host.host.as_str()))
            .collect();
        let (ca_params, ca_key, ca_pem) = generate_ca(session, &host_names)?;
        let issuer = Issuer::from_params(&ca_params, &ca_key);
        let mut hosts = BTreeMap::new();
        for (credential, value) in interception.credentials.iter().zip(values) {
            for bound in &credential.hosts {
                let server = server_config(&provider, &issuer, &bound.host)?;
                let added =
                    Credential::new(credential.name.clone(), bound.scheme.clone(), value.clone())?;
                hosts.insert(
                    bound.host.clone(),
                    BoundHost {
                        credential: added,
                        server,
                    },
                );
            }
        }
        let mut bundle = system_bundle;
        if !bundle.ends_with(b"\n") {
            bundle.push(b'\n');
        }
        bundle.extend_from_slice(ca_pem.as_bytes());
        replace_file(&interception.ca_file, ca_pem.as_bytes())?;
        replace_file(&interception.bundle_file, &bundle)?;
        Ok(Authority {
            hosts,
            upstream: Arc::new(upstream),
        })
    }

    /// The bound host `host` names, a normalized name; None for a plain tunnel.
    pub fn host(&self, host: &str) -> Option<&BoundHost> {
        self.hosts.get(host)
    }

    pub fn upstream(&self) -> Arc<rustls::ClientConfig> {
        Arc::clone(&self.upstream)
    }
}

/// A CA allowed to sign server certificates for `hosts` and names under them alone (a DNS
/// name constraint covers every depth below its name), and no intermediate CA.
fn generate_ca(
    session: &str,
    hosts: &[&str],
) -> Result<(CertificateParams, KeyPair, String), String> {
    let failed = |error: rcgen::Error| format!("cannot generate the session CA: {error}");
    let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).map_err(failed)?;
    let mut params = CertificateParams::default();
    let mut name = DistinguishedName::new();
    name.push(DnType::OrganizationName, "Torlek");
    name.push(
        DnType::CommonName,
        format!("Torlek session {session} CA").as_str(),
    );
    params.distinguished_name = name;
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    params.name_constraints = Some(NameConstraints {
        permitted_subtrees: hosts
            .iter()
            .map(|host| GeneralSubtree::DnsName((*host).to_owned()))
            .collect(),
        excluded_subtrees: Vec::new(),
    });
    set_validity(&mut params);
    let certificate = params.self_signed(&key).map_err(failed)?;
    let pem = certificate.pem();
    Ok((params, key, pem))
}

/// The server side of `host`'s tunnels: a fresh key and a certificate the CA signs for it.
fn server_config(
    provider: &Arc<rustls::crypto::CryptoProvider>,
    issuer: &Issuer<'_, &KeyPair>,
    host: &str,
) -> Result<Arc<rustls::ServerConfig>, String> {
    let failed = |error: String| format!("cannot issue the certificate for {host}: {error}");
    let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)
        .map_err(|error| failed(error.to_string()))?;
    let mut params =
        CertificateParams::new(vec![host.to_owned()]).map_err(|error| failed(error.to_string()))?;
    let mut name = DistinguishedName::new();
    name.push(DnType::CommonName, host);
    params.distinguished_name = name;
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    params.use_authority_key_identifier_extension = true;
    set_validity(&mut params);
    let certificate = params
        .signed_by(&key, issuer)
        .map_err(|error| failed(error.to_string()))?;
    let private_key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der()));
    let mut config = rustls::ServerConfig::builder_with_provider(Arc::clone(provider))
        .with_safe_default_protocol_versions()
        .map_err(|error| failed(error.to_string()))?
        .with_no_client_auth()
        .with_single_cert(vec![certificate.der().clone()], private_key)
        .map_err(|error| failed(error.to_string()))?;
    config.alpn_protocols = vec![HTTP_1_1.to_vec()];
    Ok(Arc::new(config))
}

/// The client side towards upstream servers: the host's roots, HTTP/1.1.
fn upstream_config(
    provider: &Arc<rustls::crypto::CryptoProvider>,
    bundle: &[u8],
    bundle_path: &Path,
) -> Result<rustls::ClientConfig, String> {
    let certificates: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(bundle)
        .filter_map(Result::ok)
        .collect();
    let mut roots = rustls::RootCertStore::empty();
    let (added, _ignored) = roots.add_parsable_certificates(certificates);
    if added == 0 {
        return Err(format!(
            "the host's CA bundle {} holds no usable certificate",
            bundle_path.display()
        ));
    }
    let mut config = rustls::ClientConfig::builder_with_provider(Arc::clone(provider))
        .with_safe_default_protocol_versions()
        .map_err(|error| format!("cannot configure TLS to upstream servers: {error}"))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![HTTP_1_1.to_vec()];
    Ok(config)
}

fn set_validity(params: &mut CertificateParams) {
    let today = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
        / 86_400;
    let today = i64::try_from(today).unwrap_or(0);
    let (year, month, day) = civil_date(today - 1);
    params.not_before = rcgen::date_time_ymd(year, month, day);
    let (year, month, day) = civil_date(today + VALID_DAYS);
    params.not_after = rcgen::date_time_ymd(year, month, day);
}

/// The proleptic Gregorian date `days` after 1970-01-01 (Howard Hinnant's `civil_from_days`).
fn civil_date(days: i64) -> (i32, u8, u8) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (
        i32::try_from(year).unwrap_or(1970),
        u8::try_from(month).unwrap_or(1),
        u8::try_from(day).unwrap_or(1),
    )
}

/// Writes `contents` to a new file at `path`, replacing one a former launch left. Not a
/// secret: a certificate and the host's public roots.
fn replace_file(path: &Path, contents: &[u8]) -> Result<(), String> {
    let failed = |error: std::io::Error| format!("cannot write {}: {error}", path.display());
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(failed(error)),
        _ => {}
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .open(path)
        .map_err(failed)?;
    file.write_all(contents).map_err(failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates_match_known_days() {
        assert_eq!(civil_date(0), (1970, 1, 1));
        assert_eq!(civil_date(-1), (1969, 12, 31));
        // 2000-03-01, after a leap day of a century divisible by 400.
        assert_eq!(civil_date(11_017), (2000, 3, 1));
        assert_eq!(civil_date(20_762), (2026, 11, 5));
    }
}
