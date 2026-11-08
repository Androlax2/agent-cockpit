//! `Sandbox::prepare`, the one entry every launch goes through, and the evidence it records:
//! `agents.sandbox = "off"` runs the agent unchanged and says so; `"required"` runs it through
//! the native backend (#829) or, on a host without one, refuses the launch naming why.

mod common;

use std::ffi::OsString;
use std::path::PathBuf;

use cockpit_sandbox::{
    AgentSandbox, AppliedRule, DesktopSession, Enforcement, Evidence, HostPaths, LaunchRequest,
    MIGRATIONS, NeedsSource, Platform, Prepared, Protected, ProviderNeeds, Sandbox, SecretHandling,
    WhenAbsent,
};
use common::{
    HOMES, MOUNT_SOURCES, NO_BUBBLEWRAP, OWNER_UID, launch_request, missing_backend,
    native_backend, no_host_paths,
};

fn agent_words() -> (PathBuf, Vec<OsString>) {
    (PathBuf::from("/usr/bin/claude"), vec!["--resume".into()])
}

fn sandboxes() -> [(&'static str, Sandbox); 3] {
    [
        (
            "linux",
            Sandbox::new(Platform::Linux, no_host_paths(), missing_backend()),
        ),
        (
            "macos",
            Sandbox::new(Platform::MacOs, no_host_paths(), missing_backend()),
        ),
        ("host paths unread", Sandbox::unavailable()),
    ]
}

/// The security rule of #828: an explicit `required` never falls back to running the agent
/// unsandboxed, whatever the host.
#[test]
fn a_required_sandbox_without_a_backend_refuses_every_launch_naming_why() {
    for (case, sandbox) in sandboxes() {
        let (program, arguments) = agent_words();

        let refused = sandbox
            .prepare(
                AgentSandbox::Required,
                &launch_request("s1", "/home/owner/app"),
                program,
                arguments,
            )
            .unwrap_err();

        assert_eq!(refused.rule, "agents.sandbox = \"required\"", "{case}");
        assert!(
            refused.reason.contains("no sandbox backend on this host"),
            "{case}: {refused}"
        );
        if case != "host paths unread" {
            assert!(refused.reason.contains(NO_BUBBLEWRAP), "{case}: {refused}");
        }
        assert!(
            refused
                .reason
                .contains("refused rather than run unsandboxed"),
            "{case}: {refused}"
        );
    }
}

fn native_launch(request: &LaunchRequest) -> Result<Prepared, cockpit_sandbox::Refusal> {
    let (program, arguments) = agent_words();
    Sandbox::new(Platform::Linux, no_host_paths(), native_backend()).prepare(
        AgentSandbox::Required,
        request,
        program,
        arguments,
    )
}

/// #829: the daemon runs the launcher in the agent's place; the agent's own words travel in
/// the spec, which the daemon writes before the launch.
#[test]
fn a_required_sandbox_runs_the_launcher_with_the_sessions_spec() {
    let prepared = native_launch(&launch_request("s1", "/home/owner/app")).unwrap();

    assert_eq!(
        prepared.program,
        PathBuf::from("/opt/torlek/torlek-sandbox")
    );
    assert_eq!(
        prepared.arguments,
        vec![
            OsString::from("run"),
            OsString::from(format!("{MOUNT_SOURCES}/s1/launch-spec.json"))
        ]
    );
    let spec = prepared.staged.unwrap().spec;
    assert_eq!(
        (spec.program, spec.arguments),
        (
            PathBuf::from("/usr/bin/claude"),
            vec!["--resume".to_owned()]
        )
    );
}

/// A session working in a folder of its checkout starts there, as it does unsandboxed.
#[test]
fn a_required_sandbox_starts_the_agent_in_the_sessions_working_folder() {
    let mut request = launch_request("s1", "/home/owner/app");
    request.start_dir = PathBuf::from("/home/owner/app/packages/api");

    let spec = native_launch(&request).unwrap().staged.unwrap().spec;

    assert_eq!(
        spec.working_dir,
        PathBuf::from("/home/owner/app/packages/api")
    );
}

/// #858: the agent's home is the session's own folder, standing in at the owner's `HOME`.
#[test]
fn a_required_sandbox_gives_the_session_its_own_home() {
    let spec = native_launch(&launch_request("s1", "/home/owner/app"))
        .unwrap()
        .staged
        .unwrap()
        .spec;

    assert_eq!(
        (spec.home, spec.home_source),
        (
            PathBuf::from("/home/owner"),
            PathBuf::from(format!("{HOMES}/s1"))
        )
    );
}

/// #829: Git and Torlek act on these outside the sandbox, so an absent one gets an empty
/// read-only folder; an editor's file is protected only when present.
#[test]
fn a_required_sandbox_protects_the_checkouts_git_and_torlek_folders() {
    let spec = native_launch(&launch_request("s1", "/home/owner/app"))
        .unwrap()
        .staged
        .unwrap()
        .spec;

    for (path, when_absent) in [
        ("/home/owner/app/.git/hooks", WhenAbsent::EmptyFolder),
        ("/home/owner/app/.git/config", WhenAbsent::EmptyFolder),
        ("/home/owner/app/.torlek", WhenAbsent::EmptyFolder),
        ("/home/owner/app/.envrc", WhenAbsent::Unprotected),
    ] {
        let expected = Protected {
            path: PathBuf::from(path),
            when_absent,
        };
        assert!(spec.protected.contains(&expected), "{expected:?}");
    }
}

#[test]
fn the_native_evidence_names_the_backend_bubblewrap_and_the_landlock_abi() {
    let evidence = native_launch(&launch_request("s1", "/home/owner/app"))
        .unwrap()
        .evidence;

    assert!(
        matches!(&evidence.enforcement, Enforcement::Sandboxed { backend, engine, .. }
            if backend == "torlek-sandbox" && engine == "bubblewrap 0.11.0"),
        "{evidence:?}"
    );
    assert!(
        evidence.observed.contains(&"landlock abi: 6".to_owned()),
        "{evidence:?}"
    );
}

/// #858: a provider's declared credentials never reach the sandbox, as a variable or a file.
#[test]
fn a_required_sandbox_drops_declared_credential_variables_and_hides_credential_files() {
    let mut request = launch_request("s1", "/home/owner/app");
    request
        .environment
        .push(("ANTHROPIC_API_KEY".into(), "sk-owner".into()));
    request.needs = ProviderNeeds {
        source: NeedsSource::Adapter {
            provider: "claude".to_owned(),
        },
        read: vec!["~/.claude".to_owned()],
        credential_files: vec!["~/.claude/.credentials.json".to_owned()],
        credential_variables: vec!["ANTHROPIC_API_KEY".to_owned()],
        egress: Vec::new(),
    };

    let prepared = native_launch(&request).unwrap();

    assert!(
        !prepared
            .environment
            .iter()
            .any(|(name, _)| name == "ANTHROPIC_API_KEY"),
        "{:?}",
        prepared.environment
    );
    let spec = prepared.staged.unwrap().spec;
    assert!(
        spec.hidden
            .contains(&PathBuf::from("/home/owner/.claude/.credentials.json")),
        "{:?}",
        spec.hidden
    );
}

/// #859: Git inside reads Torlek's configuration only: no system file, and no configuration
/// the launch's environment would add, can name a credential helper again.
#[test]
fn a_required_sandbox_gives_git_only_torleks_configuration() {
    let mut request = launch_request("s1", "/home/owner/app");
    request.environment.extend([
        ("GIT_CONFIG_COUNT".into(), "1".into()),
        ("GIT_CONFIG_KEY_0".into(), "credential.helper".into()),
        ("GIT_CONFIG_VALUE_0".into(), "store".into()),
        (
            "GIT_CONFIG_PARAMETERS".into(),
            "'credential.helper'='store'".into(),
        ),
    ]);

    let prepared = native_launch(&request).unwrap();

    let git_variables: Vec<_> = prepared
        .environment
        .iter()
        .filter(|(name, _)| name.to_string_lossy().starts_with("GIT_CONFIG"))
        .cloned()
        .collect();
    assert_eq!(
        git_variables,
        vec![
            (
                OsString::from("GIT_CONFIG_GLOBAL"),
                OsString::from(format!("{MOUNT_SOURCES}/s1/config/gitconfig"))
            ),
            (OsString::from("GIT_CONFIG_NOSYSTEM"), OsString::from("1")),
        ]
    );
}

/// #482: the session's end removes its whole folder of the daemon's mount sources, which
/// holds the launch spec, the egress files, the tool configuration and the folders it mounts.
#[test]
fn a_sessions_mount_sources_are_the_folder_its_launch_writes() {
    let sandbox = Sandbox::new(Platform::Linux, no_host_paths(), native_backend());
    let mut request = launch_request("s1", "/home/owner/app");
    request.provider = "codex".to_owned();
    let staged = native_launch(&request).unwrap().staged.unwrap();

    let removed = sandbox.session_mount_sources("s1").unwrap().unwrap();

    assert_eq!(removed, staged.mount_dir);
    assert!(staged.config_dir.starts_with(&removed));
    assert!(
        staged
            .folder_sources
            .iter()
            .all(|source| source.starts_with(&removed))
    );
}

/// #482: a relaunch finds its mounted folder empty, so a stale `codex.sock` target never
/// answers `ECONNREFUSED`; a link left in it is removed, never followed.
#[test]
fn a_launch_empties_the_folders_it_mounts_without_following_a_link() {
    let host = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("owner-file"), "kept\n").unwrap();
    let paths = HostPaths {
        mount_sources: host.path().join("mounts"),
        homes: host.path().join("homes"),
        logins: host.path().join("logins"),
        ..no_host_paths()
    };
    let (program, arguments) = agent_words();
    let mut request = launch_request("s1", "/home/owner/app");
    request.provider = "codex".to_owned();
    let staged = Sandbox::new(Platform::Linux, paths, native_backend())
        .prepare(AgentSandbox::Required, &request, program, arguments)
        .unwrap()
        .staged
        .unwrap();
    staged.write().unwrap();
    let mounted = staged.folder_sources[0].clone();
    std::fs::write(mounted.join("stale-entry"), "").unwrap();
    std::fs::create_dir(mounted.join("stale-folder")).unwrap();
    std::fs::write(mounted.join("stale-folder/entry"), "").unwrap();
    std::os::unix::fs::symlink(outside.path(), mounted.join("link-to-outside")).unwrap();

    staged.write().unwrap();

    assert_eq!(std::fs::read_dir(&mounted).unwrap().count(), 0);
    assert!(outside.path().join("owner-file").is_file());
}

/// A secret never crosses as a placeholder: a credential is bound to its hosts instead (#832).
#[test]
fn a_required_sandbox_refuses_a_placeholder_secret() {
    let mut request = launch_request("s1", "/home/owner/app");
    request.secrets[0].handling = SecretHandling::Placeholder;

    let refused = native_launch(&request).unwrap_err();

    assert_eq!(refused.rule, "secret TORLEK_LAUNCH_TOKEN as a placeholder");
}

#[test]
fn a_required_sandbox_refuses_a_shared_desktop_session() {
    let mut request = launch_request("s1", "/home/owner/app");
    request.desktop = DesktopSession::Shared;
    request.environment.push(("DISPLAY".into(), ":0".into()));

    let refused = native_launch(&request).unwrap_err();

    assert_eq!(refused.rule, "desktop session shared");
}

#[test]
fn the_off_setting_runs_the_agent_unchanged_and_the_evidence_says_no_sandbox() {
    let sandbox = Sandbox::new(Platform::Linux, no_host_paths(), missing_backend());
    let (program, arguments) = agent_words();
    let request = launch_request("s1", "/home/owner/app");

    let prepared = sandbox
        .prepare(
            AgentSandbox::Off,
            &request,
            program.clone(),
            arguments.clone(),
        )
        .unwrap();

    assert_eq!((prepared.program, prepared.arguments), (program, arguments));
    assert_eq!(prepared.environment, request.environment);
    assert_eq!(
        prepared.evidence,
        Evidence {
            enforcement: Enforcement::Unsandboxed {
                reason: "no sandbox: agents.sandbox = \"off\"".to_owned()
            },
            platform: "linux".to_owned(),
            rules: Vec::new(),
            observed: Vec::new(),
        }
    );
}

#[test]
fn a_hosts_denied_path_reaches_the_profile() {
    let sandbox = Sandbox::new(
        Platform::Linux,
        HostPaths {
            read_only: vec![PathBuf::from("/run/user/1000/cockpit/agent")],
            denied: vec![PathBuf::from("/run/user/1000/cockpit/daemon.sock")],
            denied_write: Vec::new(),
            owner_uid: OWNER_UID,
            mount_sources: PathBuf::from(MOUNT_SOURCES),
            sockets: Vec::new(),
            homes: PathBuf::from(HOMES),
            logins: PathBuf::from(HOMES).with_file_name("logins"),
        },
        missing_backend(),
    );

    let profile = sandbox
        .profile(&launch_request("s1", "/home/owner/app"))
        .unwrap();

    assert!(
        profile
            .rules()
            .contains(&"deny /run/user/1000/cockpit/daemon.sock".to_owned())
    );
}

#[test]
fn a_launch_requests_own_denied_path_reaches_its_profile_beside_the_hosts() {
    let sandbox = Sandbox::new(
        Platform::Linux,
        HostPaths {
            read_only: vec![PathBuf::from("/run/user/1000/cockpit/agent")],
            denied: vec![PathBuf::from("/run/user/1000/cockpit/daemon.sock")],
            denied_write: Vec::new(),
            owner_uid: OWNER_UID,
            mount_sources: PathBuf::from(MOUNT_SOURCES),
            sockets: Vec::new(),
            homes: PathBuf::from(HOMES),
            logins: PathBuf::from(HOMES).with_file_name("logins"),
        },
        missing_backend(),
    );
    let mut request = launch_request("s1", "/home/owner/app");
    request.denied = vec![PathBuf::from("/home/owner/.config/gh")];

    let rules = sandbox.profile(&request).unwrap().rules();

    for rule in [
        "deny /run/user/1000/cockpit/daemon.sock",
        "deny /home/owner/.config/gh",
    ] {
        assert!(rules.contains(&rule.to_owned()), "{rule} in {rules:?}");
    }
}

#[test]
fn a_sandbox_that_read_no_host_paths_refuses_a_profile() {
    let refused = Sandbox::unavailable()
        .profile(&launch_request("s1", "/home/owner/app"))
        .unwrap_err();

    assert_eq!(refused.rule, "sandbox");
}

/// A launch an earlier release sandboxed keeps its evidence readable after the upgrade.
#[tokio::test(flavor = "multi_thread")]
async fn the_evidence_is_read_back_as_recorded_for_the_sessions_latest_launch() {
    let dir = tempfile::tempdir().unwrap();
    let database = cockpit_turso::Database::open(&dir.path().join("state.db"), &[MIGRATIONS])
        .await
        .unwrap();
    let unsandboxed = Sandbox::new(Platform::Linux, no_host_paths(), missing_backend())
        .prepare(
            AgentSandbox::Off,
            &launch_request("s1", "/home/owner/app"),
            "/usr/bin/claude".into(),
            Vec::new(),
        )
        .unwrap()
        .evidence;
    let earlier_release = Evidence {
        enforcement: Enforcement::Sandboxed {
            backend: "an earlier backend".to_owned(),
            engine: "engine 1.0".to_owned(),
            client_identity: "cockpit-s1-0001".to_owned(),
        },
        platform: "linux".to_owned(),
        rules: vec![AppliedRule {
            rule: "read-write /home/owner/app".to_owned(),
            enforced_by: "the earlier backend".to_owned(),
        }],
        observed: vec!["landlock".to_owned()],
    };
    let write = database.write().await.unwrap();
    cockpit_sandbox::record(&write, 1, "s1", &earlier_release)
        .await
        .unwrap();
    cockpit_sandbox::record(&write, 2, "s1", &unsandboxed)
        .await
        .unwrap();
    cockpit_sandbox::record(&write, 3, "s2", &earlier_release)
        .await
        .unwrap();
    write.commit().await.unwrap();

    let read = database.read().await.unwrap();
    let latest = cockpit_sandbox::latest_for_session(&read, "s1")
        .await
        .unwrap();
    let earlier = cockpit_sandbox::latest_for_session(&read, "s2")
        .await
        .unwrap();
    let none = cockpit_sandbox::latest_for_session(&read, "s3")
        .await
        .unwrap();

    assert_eq!(latest, Some(unsandboxed));
    assert_eq!(earlier, Some(earlier_release));
    assert_eq!(none, None);
}

/// #969: the home that keeps a session's conversation is its first launch's.
#[tokio::test(flavor = "multi_thread")]
async fn a_sessions_first_launch_says_whether_it_ran_sandboxed() {
    let dir = tempfile::tempdir().unwrap();
    let database = cockpit_turso::Database::open(&dir.path().join("state.db"), &[MIGRATIONS])
        .await
        .unwrap();
    let evidence = |enforcement| Evidence {
        enforcement,
        platform: "linux".to_owned(),
        rules: Vec::new(),
        observed: Vec::new(),
    };
    let sandboxed = evidence(Enforcement::Sandboxed {
        backend: "torlek-sandbox".to_owned(),
        engine: "bubblewrap".to_owned(),
        client_identity: "cockpit-s1-0001".to_owned(),
    });
    let unsandboxed = evidence(Enforcement::Unsandboxed {
        reason: "no sandbox: agents.sandbox = \"off\"".to_owned(),
    });
    let write = database.write().await.unwrap();
    cockpit_sandbox::record(&write, 1, "s1", &unsandboxed)
        .await
        .unwrap();
    cockpit_sandbox::record(&write, 2, "s1", &sandboxed)
        .await
        .unwrap();
    cockpit_sandbox::record(&write, 3, "s2", &sandboxed)
        .await
        .unwrap();
    write.commit().await.unwrap();

    let read = database.read().await.unwrap();
    let first = |session| cockpit_sandbox::first_launch_was_sandboxed(&read, session);

    assert_eq!(first("s1").await.unwrap(), Some(false));
    assert_eq!(first("s2").await.unwrap(), Some(true));
    assert_eq!(first("s3").await.unwrap(), None);
}

fn claude_hosts() -> Vec<cockpit_sandbox::AllowEntry> {
    vec![
        cockpit_sandbox::AllowEntry {
            target: cockpit_sandbox::Target::Name("api.anthropic.com".to_owned()),
            port: 443,
            source: cockpit_sandbox::AllowSource::Adapter {
                provider: "claude".to_owned(),
            },
        },
        cockpit_sandbox::AllowEntry {
            target: cockpit_sandbox::Target::Name("index.crates.io".to_owned()),
            port: 443,
            source: cockpit_sandbox::AllowSource::Ecosystem {
                ecosystem: "rust".to_owned(),
            },
        },
    ]
}

/// #831: clients inside reach the network only through the session proxy; the owner's own
/// proxy settings would point at nothing the sandbox reaches.
#[test]
fn a_required_sandbox_sends_clients_through_the_session_proxy() {
    let mut request = launch_request("s1", "/home/owner/app");
    request
        .environment
        .push(("https_proxy".into(), "http://owner-proxy:8080".into()));

    let prepared = native_launch(&request).unwrap();

    let value = |name: &str| {
        prepared
            .environment
            .iter()
            .filter(|(variable, _)| variable == name)
            .map(|(_, value)| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
    };
    for variable in ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "ALL_PROXY"] {
        assert_eq!(value(variable), ["http://127.0.0.1:3128"], "{variable}");
    }
    assert_eq!(value("NO_PROXY"), ["localhost,127.0.0.1,::1"]);
    assert_eq!(value("NODE_USE_ENV_PROXY"), ["1"]);
}

#[test]
fn a_required_sandbox_hands_the_declared_hosts_to_the_launcher() {
    let mut request = launch_request("s1", "/home/owner/app");
    request.needs.egress = claude_hosts();

    let prepared = native_launch(&request).unwrap();

    let spec = prepared.staged.unwrap().spec;
    assert_eq!(spec.egress.entries, claude_hosts());
    assert_eq!(spec.egress.program, request.provider);
}

/// #861: the evidence names each entry's source.
#[test]
fn the_evidence_names_each_allowed_host_and_its_source() {
    let mut request = launch_request("s1", "/home/owner/app");
    request.needs.egress = claude_hosts();

    let rules: Vec<String> = native_launch(&request)
        .unwrap()
        .evidence
        .rules
        .into_iter()
        .map(|rule| rule.rule)
        .collect();

    for expected in [
        "egress: api.anthropic.com:443 (the claude adapter), TLS server name checked",
        "egress: index.crates.io:443 (the rust registry), TLS server name checked",
    ] {
        assert!(
            rules.iter().any(|rule| rule == expected),
            "{expected}: {rules:?}"
        );
    }
}

#[test]
fn the_evidence_of_a_program_declaring_no_hosts_says_so() {
    let request = launch_request("s1", "/home/owner/app");

    let rules: Vec<String> = native_launch(&request)
        .unwrap()
        .evidence
        .rules
        .into_iter()
        .map(|rule| rule.rule)
        .collect();

    let expected = format!("egress: no host, `{}` declares no hosts", request.provider);
    assert!(rules.contains(&expected), "{rules:?}");
}

/// An entry the proxy could not match as written refuses the launch.
#[test]
fn a_spec_with_a_malformed_allowlist_entry_is_refused() {
    let mut request = launch_request("s1", "/home/owner/app");
    request.needs.egress = vec![cockpit_sandbox::AllowEntry {
        target: cockpit_sandbox::Target::Name("*".to_owned()),
        port: 443,
        source: cockpit_sandbox::AllowSource::Grant {
            grant: "g1".to_owned(),
        },
    }];

    let refused = native_launch(&request).unwrap_err();

    assert_eq!(refused.rule, "launch spec");
}

/// #831: a relaunch never shows the last launch's egress counters as its own.
#[test]
fn staging_a_launch_removes_the_egress_counters_of_the_last_launch() {
    let state = tempfile::tempdir().unwrap();
    let host_paths = HostPaths {
        mount_sources: state.path().join("mounts"),
        homes: state.path().join("homes"),
        logins: state.path().join("logins"),
        ..no_host_paths()
    };
    let sandbox = Sandbox::new(Platform::Linux, host_paths, native_backend());
    let last_launch = state.path().join("mounts/s1");
    std::fs::create_dir_all(&last_launch).unwrap();
    let report = cockpit_sandbox::EgressReport::new(cockpit_sandbox::Subject::Session {
        session: "s1".to_owned(),
    });
    std::fs::write(
        last_launch.join(cockpit_sandbox::EGRESS_FILE),
        serde_json::to_vec(&report).unwrap(),
    )
    .unwrap();
    assert_eq!(sandbox.egress_report("s1").unwrap(), Some(report));
    let (program, arguments) = agent_words();
    let prepared = sandbox
        .prepare(
            AgentSandbox::Required,
            &launch_request("s1", state.path().join("app").to_str().unwrap()),
            program,
            arguments,
        )
        .unwrap();

    prepared.staged.unwrap().write().unwrap();

    assert_eq!(sandbox.egress_report("s1").unwrap(), None);
}

/// #984: a launch staging its files while the daemon rewrites the session's decisions, as its
/// resync does every second for each live session, both succeed: neither takes the other's
/// staged file from under it.
#[test]
fn staging_a_launch_while_its_decisions_are_rewritten_succeeds() {
    let state = tempfile::tempdir().unwrap();
    let host_paths = HostPaths {
        mount_sources: state.path().join("mounts"),
        homes: state.path().join("homes"),
        logins: state.path().join("logins"),
        ..no_host_paths()
    };
    let sandbox = Sandbox::new(Platform::Linux, host_paths, native_backend());
    let (program, arguments) = agent_words();
    let staged = sandbox
        .prepare(
            AgentSandbox::Required,
            &launch_request("s1", state.path().join("app").to_str().unwrap()),
            program,
            arguments,
        )
        .unwrap()
        .staged
        .unwrap();
    staged.write().unwrap();
    let denial = |request| cockpit_sandbox::EgressDecisions {
        allowed: Vec::new(),
        denied: vec![cockpit_sandbox::DeniedDestination {
            host: "denied.example.com".to_owned(),
            port: 443,
            request,
        }],
        asking: Vec::new(),
    };
    let both_started = std::sync::Barrier::new(2);

    std::thread::scope(|scope| {
        let resync = scope.spawn(|| {
            both_started.wait();
            for request in 1..=300 {
                sandbox
                    .write_egress_decisions("s1", &denial(request))
                    .unwrap_or_else(|error| panic!("rewrite {request}: {error}"));
            }
        });
        both_started.wait();
        for launch in 1..=300 {
            staged
                .write()
                .unwrap_or_else(|error| panic!("launch {launch}: {error}"));
        }
        resync.join().unwrap();
    });

    let folder = state.path().join("mounts/s1");
    let written = std::fs::read(folder.join(cockpit_sandbox::DECISIONS_FILE)).unwrap();
    serde_json::from_slice::<cockpit_sandbox::EgressDecisions>(&written).unwrap();
    let left_staged: Vec<_> = std::fs::read_dir(&folder)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .filter(|name| name.to_string_lossy().contains(".staged"))
        .collect();
    assert_eq!(left_staged, Vec::<std::ffi::OsString>::new());
}
