//! A credential bound to its hosts (#832): it reaches the launcher in its own owner-only file,
//! never in the spec or the agent's environment, which holds a placeholder and the trust
//! variables that name the session's CA. Binding one to a host the session may not reach, or
//! to an unsandboxed launch, is refused.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use cockpit_sandbox::{
    AgentSandbox, AllowEntry, AllowSource, BoundCredential, CredentialHost, CredentialScheme,
    CredentialValue, HostPaths, LaunchRequest, PLACEHOLDER, Platform, Prepared, Refusal, Sandbox,
    Target,
};
use common::{MOUNT_SOURCES, launch_request, native_backend, no_host_paths};

const SECRET: &str = "github_pat_never_inside";
const NAME: &str = "the project's GitHub API token";

fn github_hosts() -> Vec<AllowEntry> {
    ["github.com", "api.github.com"]
        .into_iter()
        .map(|host| AllowEntry {
            target: Target::Name(host.to_owned()),
            port: 443,
            source: AllowSource::Grant {
                grant: "g1".to_owned(),
            },
        })
        .collect()
}

fn api_token() -> BoundCredential {
    BoundCredential {
        name: NAME.to_owned(),
        placeholder_variable: Some("GH_TOKEN".to_owned()),
        value: CredentialValue::new(SECRET.to_owned()),
        hosts: vec![
            CredentialHost {
                host: "github.com".to_owned(),
                scheme: CredentialScheme::GitReads {
                    username: "x-access-token".to_owned(),
                },
            },
            CredentialHost {
                host: "api.github.com".to_owned(),
                scheme: CredentialScheme::Bearer,
            },
        ],
    }
}

/// A launch reaching GitHub with its API token bound, the host's CA bundle at `bundle`.
fn bound_request(bundle: &Path) -> LaunchRequest {
    let mut request = launch_request("s1", "/home/owner/app");
    request.needs.egress = github_hosts();
    request.credentials = vec![api_token()];
    request
        .environment
        .push(("SSL_CERT_FILE".into(), bundle.into()));
    request
}

/// The host's CA bundle the launcher verifies upstream servers against.
fn host_bundle() -> tempfile::NamedTempFile {
    let bundle = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(bundle.path(), "-----BEGIN CERTIFICATE-----\n").unwrap();
    bundle
}

fn prepare_in(
    sandbox: &Sandbox,
    setting: AgentSandbox,
    request: &LaunchRequest,
) -> Result<Prepared, Refusal> {
    sandbox.prepare(
        setting,
        request,
        PathBuf::from("/usr/bin/claude"),
        Vec::new(),
    )
}

fn native_launch(request: &LaunchRequest) -> Result<Prepared, Refusal> {
    let sandbox = Sandbox::new(Platform::Linux, no_host_paths(), native_backend());
    prepare_in(&sandbox, AgentSandbox::Required, request)
}

fn values_of(prepared: &Prepared, name: &str) -> Vec<String> {
    prepared
        .environment
        .iter()
        .filter(|(variable, _)| variable == name)
        .map(|(_, value)| value.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn the_spec_names_each_bound_host_and_its_scheme_but_holds_no_value() {
    let bundle = host_bundle();

    let prepared = native_launch(&bound_request(bundle.path())).unwrap();

    let spec = prepared.staged.unwrap().spec;
    let interception = spec.interception.as_ref().unwrap();
    assert_eq!(interception.credentials.len(), 1);
    assert_eq!(interception.credentials[0].name, NAME);
    assert_eq!(interception.credentials[0].hosts, api_token().hosts);
    assert_eq!(interception.system_bundle, bundle.path());
    let text = serde_json::to_string(&spec).unwrap();
    assert!(!text.contains(SECRET), "{text}");
}

#[test]
fn the_agent_holds_a_placeholder_and_never_the_value() {
    let bundle = host_bundle();
    let mut request = bound_request(bundle.path());
    request
        .environment
        .push(("GH_TOKEN".into(), "owner_token".into()));

    let prepared = native_launch(&request).unwrap();

    assert_eq!(values_of(&prepared, "GH_TOKEN"), [PLACEHOLDER]);
    for (name, value) in &prepared.environment {
        assert!(
            !value.to_string_lossy().contains(SECRET),
            "{}",
            name.to_string_lossy()
        );
    }
}

/// Each runtime's own trust variable: a replacing bundle for OpenSSL, Git, Go, Python and
/// curl; the CA alone for Node, which adds it to its own roots.
#[test]
fn the_trust_variables_name_the_sessions_trust_files() {
    let bundle = host_bundle();

    let prepared = native_launch(&bound_request(bundle.path())).unwrap();

    let config_dir = format!("{MOUNT_SOURCES}/s1/config");
    for variable in [
        "SSL_CERT_FILE",
        "GIT_SSL_CAINFO",
        "REQUESTS_CA_BUNDLE",
        "CURL_CA_BUNDLE",
    ] {
        assert_eq!(
            values_of(&prepared, variable),
            [format!("{config_dir}/ca-bundle.pem")],
            "{variable}"
        );
    }
    assert_eq!(
        values_of(&prepared, "NODE_EXTRA_CA_CERTS"),
        [format!("{config_dir}/session-ca.pem")]
    );
}

#[test]
fn a_launch_without_a_bound_credential_sets_no_trust_variable() {
    let prepared = native_launch(&launch_request("s1", "/home/owner/app")).unwrap();

    assert!(values_of(&prepared, "NODE_EXTRA_CA_CERTS").is_empty());
    assert!(prepared.staged.unwrap().spec.interception.is_none());
}

#[test]
fn staging_writes_the_values_owner_only_beside_the_spec() {
    let state = tempfile::tempdir().unwrap();
    let sandbox = state_sandbox(state.path());
    let bundle = host_bundle();
    let request = LaunchRequest {
        working_dir: state.path().join("app"),
        start_dir: state.path().join("app"),
        ..bound_request(bundle.path())
    };

    prepare_in(&sandbox, AgentSandbox::Required, &request)
        .unwrap()
        .staged
        .unwrap()
        .write()
        .unwrap();

    let file = state.path().join("mounts/s1/credentials.json");
    let values: Vec<String> = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(values, [SECRET]);
    let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

/// A relaunch after the owner removed the token leaves none of the last launch's values.
#[test]
fn staging_a_launch_without_one_removes_the_values_of_the_last_launch() {
    let state = tempfile::tempdir().unwrap();
    let sandbox = state_sandbox(state.path());
    let bundle = host_bundle();
    let bound = LaunchRequest {
        working_dir: state.path().join("app"),
        start_dir: state.path().join("app"),
        ..bound_request(bundle.path())
    };
    let stage = |request: &LaunchRequest| {
        prepare_in(&sandbox, AgentSandbox::Required, request)
            .unwrap()
            .staged
            .unwrap()
            .write()
            .unwrap();
    };
    stage(&bound);

    stage(&LaunchRequest {
        credentials: Vec::new(),
        ..bound
    });

    assert!(!state.path().join("mounts/s1/credentials.json").exists());
}

/// #857, decision 3: a binding never grants reach.
#[test]
fn a_credential_for_a_host_the_session_may_not_reach_is_refused() {
    let bundle = host_bundle();
    let mut request = bound_request(bundle.path());
    request
        .needs
        .egress
        .retain(|entry| entry.target != Target::Name("github.com".to_owned()));

    let refused = native_launch(&request).unwrap_err();

    assert_eq!(refused.rule, "launch spec");
    assert!(
        refused.reason.contains(&format!(
            "{NAME} cannot be bound to github.com: the session may not reach it"
        )),
        "{refused}"
    );
}

#[test]
fn an_unsandboxed_launch_is_refused_a_bound_credential() {
    let bundle = host_bundle();
    let sandbox = Sandbox::new(Platform::Linux, no_host_paths(), native_backend());

    let refused =
        prepare_in(&sandbox, AgentSandbox::Off, &bound_request(bundle.path())).unwrap_err();

    assert_eq!(refused.rule, format!("credential {NAME}"));
}

#[test]
fn an_owners_ssl_cert_file_that_names_no_file_is_refused() {
    let request = bound_request(Path::new("relative/bundle.pem"));

    let refused = native_launch(&request).unwrap_err();

    assert_eq!(refused.rule, "host CA bundle");
}

#[test]
fn the_evidence_names_each_bound_host_the_placeholder_and_the_trust_files() {
    let bundle = host_bundle();

    let rules: Vec<String> = native_launch(&bound_request(bundle.path()))
        .unwrap()
        .evidence
        .rules
        .into_iter()
        .map(|rule| rule.rule)
        .collect();

    for expected in [
        format!("credential: {NAME} added on github.com:443 as Basic on Git fetches only"),
        format!("credential: {NAME} added on api.github.com:443 as Bearer on every request"),
        format!(
            "credential: {NAME} never enters the sandbox, which holds a placeholder in GH_TOKEN"
        ),
        "trust: the per-session CA in NODE_EXTRA_CA_CERTS".to_owned(),
    ] {
        assert!(
            rules.iter().any(|rule| rule.starts_with(&expected)),
            "{expected}: {rules:#?}"
        );
    }
}

fn state_sandbox(state: &Path) -> Sandbox {
    Sandbox::new(
        Platform::Linux,
        HostPaths {
            mount_sources: state.join("mounts"),
            homes: state.join("homes"),
            ..no_host_paths()
        },
        native_backend(),
    )
}
