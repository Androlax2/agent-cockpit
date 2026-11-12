//! The sandbox through the real daemon binary (#828, #829). By default every launch runs
//! unsandboxed, the evidence says so and the risk is logged at the start and at each launch.
//! `agents.sandbox = "required"` runs the agent in Torlek's native sandbox (`torlek-sandbox`
//! under bubblewrap, Landlock and seccomp) and, on a host without it, refuses every launch
//! naming why and never runs an agent. `cockpit session sandbox` and `cockpit sandbox show`
//! print the evidence and the command.
//!
//! The sandboxed tests drive the [`COMMANDED_PROJECT`] agent: each line it runs shows what
//! the agent sees from inside the sandbox.

mod candidate;
mod real_daemon;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cockpit_client::{ClientError, Connection};
use cockpit_protocol::{
    Command, ErrorCode, LaunchKindView, QuotaStopStatusView, Reply, SessionStateView, SessionView,
};
use real_daemon::{
    BUILD_PROFILE_DIR, COMMANDED_PROJECT, Daemon, GENEROUS, Home, SETTLE, added_context,
    cockpit_program, is_alive, launch_option, listed, mcp_tool_text, new_session,
    session_new_command,
};

const REQUIRED: &str = "[agents]\nsandbox = \"required\"\n";
/// What [`cockpit_sandbox::Backend::probe`] names on a host without bubblewrap.
const NO_BUBBLEWRAP: &str = "bubblewrap (`bwrap`)";

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

async fn sandbox_show(home: &Home, root: &std::path::Path) -> std::process::Output {
    home.cockpit(
        &[
            "sandbox",
            "show",
            root.to_str().unwrap(),
            "--agent",
            "claude",
        ],
        &[],
    )
    .await
}

/// A sandboxed agent under `required`: the daemon, the agent's checkout and its session.
struct Sandboxed {
    _daemon: Daemon,
    root: PathBuf,
    session: String,
}

/// Starts the daemon with `required` and launches the [`COMMANDED_PROJECT`] agent, after
/// `prepare` set up the owner's home.
async fn sandboxed_agent(home: &Home, prepare: impl FnOnce(&Home)) -> Sandboxed {
    sandboxed_agent_on_path(home, &[], prepare).await
}

/// [`sandboxed_agent`] with `path_first` ahead on the daemon's and the agent's `PATH`.
async fn sandboxed_agent_on_path(
    home: &Home,
    path_first: &[PathBuf],
    prepare: impl FnOnce(&Home),
) -> Sandboxed {
    sandboxed_agent_started(home, |home| home.start_with_path_first(path_first), prepare).await
}

/// [`sandboxed_agent`] with the daemon started by `start`.
async fn sandboxed_agent_started(
    home: &Home,
    start: impl FnOnce(&Home) -> Daemon,
    prepare: impl FnOnce(&Home),
) -> Sandboxed {
    assert!(
        candidate::program("torlek-sandbox").is_file(),
        "the launcher is built beside the daemon: run the workspace's tests"
    );
    home.write_owner_config(REQUIRED);
    prepare(home);
    let daemon = start(home);
    let client = home.connect().await;
    let (root, _) = home.project(&client, COMMANDED_PROJECT).await;
    let session = new_session(&client, &root).await;
    Sandboxed {
        _daemon: daemon,
        root,
        session: session.id,
    }
}

/// What `line` printed in the sandboxed agent, ending `exit=<code>`.
async fn run_in_agent(home: &Home, root: &Path, line: &str) -> String {
    let number = home.command_agent(root, line);
    home.agent_output(root, number).await
}

#[tokio::test(flavor = "multi_thread")]
async fn by_default_a_launch_runs_unsandboxed_and_the_evidence_says_no_sandbox() {
    let home = Home::new();
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, "app").await;
    let session = new_session(&client, &root).await;
    home.wait_for_launches(&session.id, 1).await;

    let shown = home
        .cockpit(&["session", "sandbox", &session.id], &[])
        .await;

    assert!(shown.status.success(), "{}", text(&shown.stderr));
    let first = text(&shown.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned();
    assert!(
        first.starts_with("sandbox: NONE on ")
            && first.ends_with("unsandboxed: no sandbox: agents.sandbox = \"off\""),
        "{first}"
    );
}

/// The security rule of #828: `required` never degrades to an unsandboxed launch.
#[tokio::test(flavor = "multi_thread")]
async fn a_required_setting_without_bubblewrap_refuses_the_launch_naming_why() {
    let home = Home::new();
    home.write_owner_config(REQUIRED);
    let _daemon = home.start_without_bubblewrap();
    let client = home.connect().await;
    let (root, _) = home.project(&client, "app").await;

    let refused = client
        .request(session_new_command(&root), Duration::from_secs(60))
        .await;

    assert!(
        matches!(&refused, Err(ClientError::Daemon(error))
            if error.code == ErrorCode::Refused && error.message.contains(NO_BUBBLEWRAP)),
        "{refused:?}"
    );
    tokio::time::sleep(SETTLE).await;
    assert!(home.launches().is_empty(), "{:?}", home.launches());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_required_setting_without_bubblewrap_logs_at_start_that_every_launch_is_refused() {
    let home = Home::new();
    home.write_owner_config(REQUIRED);
    let _daemon = home.start_without_bubblewrap();

    let log = home
        .log_once_it_contains("every agent launch is refused")
        .await;

    assert!(
        log.lines()
            .any(|line| line.contains("every agent launch is refused")
                && line.contains(NO_BUBBLEWRAP)),
        "{log}"
    );
}

/// The daemon's start and each unsandboxed launch warn that grants no longer hold (#729).
#[tokio::test(flavor = "multi_thread")]
async fn the_default_warns_at_start_and_at_each_launch_that_an_agent_can_act_as_the_owner() {
    let home = Home::new();
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, "app").await;
    let session = new_session(&client, &root).await;
    home.wait_for_launches(&session.id, 1).await;

    let log = home
        .log_once_it_contains("an agent launches UNSANDBOXED")
        .await;

    for warning in ["agents run UNSANDBOXED", "an agent launches UNSANDBOXED"] {
        assert!(
            log.lines()
                .any(|line| line.contains(warning)
                    && line.contains(cockpit_config::UNSANDBOXED_RISK)),
            "{warning:?} missing from {log}"
        );
    }
}

/// The line `sandbox show` printed for gate `name`, and the `next:` line under it.
fn gate<'printed>(printed: &'printed str, name: &str) -> (&'printed str, Option<&'printed str>) {
    let mut lines = printed.lines();
    let line = lines
        .by_ref()
        .find(|line| line.contains(&format!("  launch {name}: ")))
        .unwrap_or_else(|| panic!("no launch {name} gate in:\n{printed}"));
    let next = lines
        .next()
        .and_then(|line| line.trim_start().strip_prefix("next: "));
    (line, next)
}

#[tokio::test(flavor = "multi_thread")]
async fn cockpit_sandbox_show_prints_no_sandbox_and_the_providers_command_without_launching() {
    let home = Home::new();
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, "app").await;

    let shown = sandbox_show(&home, &root).await;

    assert!(shown.status.success(), "{}", text(&shown.stderr));
    let printed = text(&shown.stdout);
    let (backend, _) = gate(&printed, "backend");
    assert!(backend.starts_with("available "), "{printed}");
    assert!(printed.contains("\nsandbox: NONE on "), "{printed}");
    let claude = home.path().join("bin/claude");
    assert!(
        printed.contains(&format!("\ncommand: {} ", claude.display())),
        "{printed}"
    );
    assert!(home.launches().is_empty());
}

/// #1174: under `required` on a host with the backend, the dry run reports it and the
/// launcher's command, instead of refusing on its own stand-in for the agent's program.
#[tokio::test(flavor = "multi_thread")]
async fn cockpit_sandbox_show_with_a_required_setting_reports_the_backend_ready() {
    assert!(
        candidate::program("torlek-sandbox").is_file(),
        "the launcher is built beside the daemon: run the workspace's tests"
    );
    let home = Home::new();
    home.write_owner_config(REQUIRED);
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, "app").await;

    let shown = sandbox_show(&home, &root).await;

    let printed = text(&shown.stdout);
    assert!(shown.status.success(), "{printed}{}", text(&shown.stderr));
    let (backend, _) = gate(&printed, "backend");
    assert!(
        backend.starts_with("available ") && backend.contains("torlek-sandbox"),
        "{printed}"
    );
    let (profile, _) = gate(&printed, "profile");
    assert!(profile.starts_with("available "), "{printed}");
    assert!(
        printed.contains("\nsandbox: enforced by torlek-sandbox on "),
        "{printed}"
    );
    assert!(home.launches().is_empty());
}

/// #1250: the gate that refuses is one line with its fix, beside the gates that pass.
#[tokio::test(flavor = "multi_thread")]
async fn cockpit_sandbox_show_with_a_required_setting_without_bubblewrap_names_the_gate_and_its_fix()
 {
    let home = Home::new();
    home.write_owner_config(REQUIRED);
    let _daemon = home.start_without_bubblewrap();
    let client = home.connect().await;
    let (root, _) = home.project(&client, "app").await;

    let shown = sandbox_show(&home, &root).await;

    assert_eq!(shown.status.code(), Some(1), "{}", text(&shown.stderr));
    let printed = text(&shown.stdout);
    let (backend, next) = gate(&printed, "backend");
    assert!(
        backend.starts_with("missing-prerequisite ") && backend.contains(NO_BUBBLEWRAP),
        "{printed}"
    );
    assert!(
        next.is_some_and(|next| next.contains("agents.sandbox = \"off\"")),
        "{printed}"
    );
    let (command, _) = gate(&printed, "command");
    assert!(command.starts_with("available "), "{printed}");
    assert!(!printed.contains("\nsandbox: "), "{printed}");
}

/// #1250: `doctor --project` names the launch gate that refuses a session there.
#[tokio::test(flavor = "multi_thread")]
async fn doctor_for_a_project_names_the_launch_gate_that_refuses() {
    let home = Home::new();
    home.write_owner_config(REQUIRED);
    let _daemon = home.start_without_bubblewrap();
    let client = home.connect().await;
    let (root, _) = home.project(&client, "app").await;

    let doctor = home
        .cockpit(&["doctor", "--project", root.to_str().unwrap()], &[])
        .await;

    let report = text(&doctor.stdout);
    assert_eq!(doctor.status.code(), Some(1), "{report}");
    let (backend, _) = gate(&report, "backend");
    assert!(backend.starts_with("missing-prerequisite "), "{report}");
}

/// #1250: with every gate passing, `doctor --project` says so in one line.
#[tokio::test(flavor = "multi_thread")]
async fn doctor_for_a_project_whose_launch_passes_says_so_in_one_line() {
    let home = Home::new();
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, "app").await;

    let doctor = home
        .cockpit(&["doctor", "--project", root.to_str().unwrap()], &[])
        .await;

    let report = text(&doctor.stdout);
    let line = report
        .lines()
        .find(|line| line.contains("  launch: "))
        .unwrap_or_else(|| panic!("no launch line in:\n{report}"));
    assert!(
        line.starts_with("available ") && line.contains("passes every launch gate"),
        "{report}"
    );
    assert!(!report.contains("  launch backend: "), "{report}");
}

/// #829: the evidence names the backend, bubblewrap's version and the Landlock ABI.
#[tokio::test(flavor = "multi_thread")]
async fn a_required_launch_runs_in_the_native_sandbox_and_the_evidence_names_it() {
    let home = Home::new();
    let agent = sandboxed_agent(&home, |_| {}).await;
    run_in_agent(&home, &agent.root, "true").await;

    let shown = home
        .cockpit(&["session", "sandbox", &agent.session], &[])
        .await;

    assert!(shown.status.success(), "{}", text(&shown.stderr));
    let printed = text(&shown.stdout);
    assert!(
        printed.starts_with("sandbox: enforced by torlek-sandbox on "),
        "{printed}"
    );
    assert!(printed.contains("\nengine: bubblewrap "), "{printed}");
    assert!(printed.contains("\nobserved: landlock abi: "), "{printed}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sandboxed_agent_writes_its_checkout() {
    let home = Home::new();
    let agent = sandboxed_agent(&home, |_| {}).await;

    let printed = run_in_agent(&home, &agent.root, "echo written > written && cat written").await;

    assert_eq!(printed, "written\nexit=0\n");
    assert_eq!(
        std::fs::read_to_string(agent.root.join("written")).unwrap(),
        "written\n"
    );
}

/// #829: hooks and Git's configuration run code on the owner's next Git command, and
/// `.torlek` holds what the owner configured for the project.
#[tokio::test(flavor = "multi_thread")]
async fn a_sandboxed_agent_may_not_write_git_hooks_git_config_or_torlek() {
    let home = Home::new();
    let agent = sandboxed_agent(&home, |_| {}).await;

    for protected in [
        ".git/hooks/pre-commit",
        ".git/config",
        ".torlek/project.toml",
    ] {
        let printed = run_in_agent(
            &home,
            &agent.root,
            &format!("echo planted >> {protected} || echo refused"),
        )
        .await;

        assert!(
            printed.ends_with("refused\nexit=0\n"),
            "{protected}: {printed}"
        );
        assert!(
            !std::fs::read_to_string(agent.root.join(protected))
                .unwrap_or_default()
                .contains("planted"),
            "{protected}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sandboxed_agent_may_not_read_the_owners_ssh_keys_or_the_state_database() {
    let home = Home::new();
    let agent = sandboxed_agent(&home, |home| {
        let ssh = home.path().join(".ssh");
        std::fs::create_dir_all(&ssh).unwrap();
        std::fs::write(ssh.join("id_ed25519"), "owner's key\n").unwrap();
    })
    .await;
    let database = home.state_dir().join("state.db");
    assert!(database.is_file());

    for unreadable in [home.path().join(".ssh/id_ed25519"), database] {
        let printed = run_in_agent(
            &home,
            &agent.root,
            &format!("cat '{}' || echo refused", unreadable.display()),
        )
        .await;

        assert!(
            printed.ends_with("refused\nexit=0\n"),
            "{unreadable:?}: {printed}"
        );
        assert!(!printed.contains("owner's key"), "{printed}");
    }
}

/// A sandboxed agent's network is its own loopback: the session proxy (#831) listens there
/// and is its only way out.
#[tokio::test(flavor = "multi_thread")]
async fn a_sandboxed_agent_sees_only_the_loopback_interface() {
    let home = Home::new();
    let agent = sandboxed_agent(&home, |_| {}).await;

    let printed = run_in_agent(
        &home,
        &agent.root,
        "tail -n +3 /proc/net/dev | cut -d: -f1 | tr -d ' '",
    )
    .await;

    assert_eq!(printed, "lo\nexit=0\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sandboxed_agent_runs_under_a_seccomp_filter() {
    let home = Home::new();
    let agent = sandboxed_agent(&home, |_| {}).await;

    let printed = run_in_agent(&home, &agent.root, "grep '^Seccomp:' /proc/self/status").await;

    assert_eq!(printed, "Seccomp:\t2\nexit=0\n");
}

/// #628: a sandbox that kept its filter at a fixed path in the shared `/tmp` started for the
/// first Unix user only. CI's greenfield job runs this as a second user after the gate ran
/// sandboxed sessions as the runner's user, whose id `COCKPIT_FIRST_USER_UID` names.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "CI's greenfield job runs it as a second Unix user (#628)"]
async fn a_second_unix_user_starts_a_sandboxed_session() {
    let first_user: u32 = std::env::var("COCKPIT_FIRST_USER_UID")
        .expect("COCKPIT_FIRST_USER_UID names the user who ran sandboxed sessions first")
        .parse()
        .expect("COCKPIT_FIRST_USER_UID is a user id");
    let home = Home::new();
    let user = std::os::unix::fs::MetadataExt::uid(&std::fs::metadata(home.path()).unwrap());
    assert_ne!(user, first_user, "run as a second Unix user");
    let agent = sandboxed_agent(&home, |_| {}).await;

    let printed = run_in_agent(&home, &agent.root, "grep '^Seccomp:' /proc/self/status").await;

    assert_eq!(printed, "Seccomp:\t2\nexit=0\n");
}

/// #857: under `required` the project's token stays with the daemon; the owner's `gh`
/// sign-in is in a home the agent does not see.
#[tokio::test(flavor = "multi_thread")]
async fn a_sandboxed_agent_gets_no_github_token_or_gh_sign_in() {
    let home = Home::new();
    let agent = sandboxed_agent(&home, |home| {
        home.write_github_token(COMMANDED_PROJECT, "ghp_project_token\n", 0o600);
        let gh = home.path().join(".config/gh");
        std::fs::create_dir_all(&gh).unwrap();
        std::fs::write(gh.join("hosts.yml"), "oauth_token: owner\n").unwrap();
    })
    .await;

    let printed = run_in_agent(
        &home,
        &agent.root,
        "echo \"${GH_TOKEN-unset} ${GITHUB_TOKEN-unset}\"; cat ~/.config/gh/hosts.yml || echo refused",
    )
    .await;

    assert!(printed.starts_with("unset unset\n"), "{printed}");
    assert!(printed.ends_with("refused\nexit=0\n"), "{printed}");
}

/// #857 decision 1: with both of the project's GitHub tokens kept, the push token still
/// stays with the daemon: neither the agent's environment nor its home, checkout or temporary
/// folders hold its value. A file the agent writes with it first shows the scan finds one.
#[tokio::test(flavor = "multi_thread")]
async fn with_both_github_tokens_kept_the_push_token_is_in_no_folder_the_agent_sees() {
    let home = Home::new();
    let agent = sandboxed_agent(&home, |home| {
        home.write_github_token(COMMANDED_PROJECT, "ghp_push_token\n", 0o600);
        home.write_github_api_token(COMMANDED_PROJECT, "github_pat_api_token\n", 0o600);
    })
    .await;

    let printed = run_in_agent(
        &home,
        &agent.root,
        // The value is spelled in two halves: the command's own script is in the home too.
        "value=ghp_push; value=${value}_token; \
         cat /proc/self/environ | tr '\\0' '\\n' | grep -c \"$value\"; \
         printf '%s' \"$value\" > push-token-control; \
         grep -RlsD skip \"$value\" ~ \"$PWD\" \"${TMPDIR:-/tmp}\" /tmp",
    )
    .await;

    let mut lines = printed.lines();
    assert_eq!(lines.next(), Some("0"), "{printed}");
    assert!(printed.ends_with("exit=0\n"), "{printed}");
    let found: Vec<&str> = lines.filter(|line| !line.starts_with("exit=")).collect();
    assert!(
        !found.is_empty(),
        "the scan found not even the control: {printed}"
    );
    assert!(
        found
            .iter()
            .all(|path| path.ends_with("/push-token-control")),
        "{printed}"
    );
}

/// Every provider adapter's credential paths, `~/` stripped.
fn providers_credential_files() -> Vec<&'static str> {
    cockpit_claude::needs::CREDENTIAL_FILES
        .iter()
        .chain(&cockpit_codex::needs::CREDENTIAL_FILES)
        .chain(&cockpit_opencode::needs::CREDENTIAL_FILES)
        .map(|path| path.strip_prefix("~/").expect("a path in the home"))
        .collect()
}

/// #858, and #830's daemon acceptance: no provider's sign-in written in the owner's home is
/// in the view, the launched provider's included, whose folders are the session's own. The
/// launch's masking of a credential file inside them is `cockpit-sandbox`'s test.
#[tokio::test(flavor = "multi_thread")]
async fn a_sandboxed_agent_sees_no_providers_credential_file() {
    let home = Home::new();
    let agent = sandboxed_agent(&home, |home| {
        for file in providers_credential_files() {
            let path = home.path().join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "owner's sign-in\n").unwrap();
        }
    })
    .await;

    for file in providers_credential_files() {
        let printed = run_in_agent(
            &home,
            &agent.root,
            &format!("test -e ~/{file} && echo present || echo absent"),
        )
        .await;

        assert_eq!(printed, "absent\nexit=0\n", "{file}");
    }
}

/// #858: the owner's provider keys in the daemon's environment reach no agent; until #832's
/// placeholders, it gets none. The daemon passes an agent only its base variables; the
/// launcher's drop of a provider's declared variables is `cockpit-sandbox`'s test.
#[tokio::test(flavor = "multi_thread")]
async fn a_sandboxed_agent_gets_no_providers_credential_variable() {
    let home = Home::new();
    let owner_keys: Vec<(&str, &str)> = cockpit_claude::needs::CREDENTIAL_VARIABLES
        .iter()
        .chain(&cockpit_codex::needs::CREDENTIAL_VARIABLES)
        .copied()
        .chain(
            cockpit_opencode::needs::CREDENTIAL_BINDINGS
                .iter()
                .map(|(variable, _, _)| *variable),
        )
        .map(|variable| (variable, "owner-key"))
        .collect();
    let agent = sandboxed_agent_started(&home, |home| home.start_with(&owner_keys), |_| {}).await;

    let printed = run_in_agent(&home, &agent.root, "env | grep -c owner-key").await;

    assert_eq!(printed, "0\nexit=1\n");
}

/// #832: the project's API token is bound only to the GitHub hosts the session reaches, and
/// the claude adapter declares none: the evidence names both hosts as left without it, and
/// the agent holds neither token nor placeholder.
#[tokio::test(flavor = "multi_thread")]
async fn an_api_token_for_hosts_the_session_does_not_reach_is_named_and_not_added() {
    let home = Home::new();
    let agent = sandboxed_agent(&home, |home| {
        home.write_github_api_token(COMMANDED_PROJECT, "github_pat_api_token\n", 0o600);
    })
    .await;

    let printed = run_in_agent(&home, &agent.root, "echo \"${GH_TOKEN-unset}\"").await;
    let shown = home
        .cockpit(&["session", "sandbox", &agent.session], &[])
        .await;

    assert_eq!(printed, "unset\nexit=0\n");
    let evidence = text(&shown.stdout);
    for host in ["github.com", "api.github.com"] {
        assert!(
            evidence.contains(&format!(
                "credential: the project's GitHub API token is not added on {host}:443: the \
                 session's launch does not reach it"
            )),
            "{evidence}"
        );
    }
}

/// #857: the session proxy allows no forge the adapter does not declare, so a push from
/// inside reaches none.
#[tokio::test(flavor = "multi_thread")]
async fn a_sandboxed_agents_git_push_fails() {
    let home = Home::new();
    let agent = sandboxed_agent(&home, |_| {}).await;

    let printed = run_in_agent(
        &home,
        &agent.root,
        "GIT_TERMINAL_PROMPT=0 git push --quiet https://github.com/Androlax2/agent-cockpit.git \
         HEAD:refs/heads/sandbox-test >/dev/null 2>&1; echo \"push=$?\"",
    )
    .await;

    assert!(
        printed.starts_with("push=") && !printed.starts_with("push=0\n"),
        "{printed}"
    );
}

/// #434: the launcher removes the spec it read, so no launch leaves one behind.
#[tokio::test(flavor = "multi_thread")]
async fn a_sandboxed_launch_leaves_no_launch_spec() {
    let home = Home::new();
    let agent = sandboxed_agent(&home, |_| {}).await;
    run_in_agent(&home, &agent.root, "true").await;

    let spec = home
        .path()
        .join("run")
        .join(BUILD_PROFILE_DIR)
        .join("mounts")
        .join(&agent.session)
        .join(cockpit_sandbox::SPEC_FILE);

    assert!(!spec.exists(), "{spec:?}");
    assert!(spec.parent().unwrap().is_dir(), "{spec:?}");
}

/// #856: no container engine socket, nor the variables that point at one. `DOCKER_CONFIG`
/// names a folder for the client's settings, not an engine (#1253), so it is not one. #390:
/// nor the project engine's, which a launch reaches only once its checkout is masked.
#[tokio::test(flavor = "multi_thread")]
async fn a_sandboxed_agent_reaches_no_container_engine() {
    let home = Home::new();
    let agent = sandboxed_agent(&home, |_| {}).await;

    let printed = run_in_agent(
        &home,
        &agent.root,
        "for socket in /var/run/docker.sock /run/docker.sock /run/podman/podman.sock \
         /run/containerd/containerd.sock /run/torlek-engine/docker.sock \
         \"${XDG_RUNTIME_DIR-/nonexistent}/docker.sock\" \
         \"${XDG_RUNTIME_DIR-/nonexistent}/podman/podman.sock\"; do \
         test -e \"$socket\" && echo \"present: $socket\"; done; \
         env | grep '^DOCKER_' | grep -v '^DOCKER_CONFIG=' || echo 'no docker variables'",
    )
    .await;

    assert_eq!(printed, "no docker variables\nexit=0\n");
}

/// #858: the agent's home is the session's own, kept across relaunches, never the owner's.
#[tokio::test(flavor = "multi_thread")]
async fn a_sandboxed_agents_home_is_the_sessions_own_folder() {
    let home = Home::new();
    let agent = sandboxed_agent(&home, |home| {
        std::fs::write(home.path().join("owner-file"), "owner\n").unwrap();
    })
    .await;

    let printed = run_in_agent(
        &home,
        &agent.root,
        "test -e ~/owner-file && echo visible; echo kept > ~/agent-file && echo written",
    )
    .await;

    assert_eq!(printed, "written\nexit=0\n");
    let session_home = home
        .state_dir()
        .join("homes")
        .join(&agent.session)
        .join("agent-file");
    assert_eq!(std::fs::read_to_string(session_home).unwrap(), "kept\n");
}

/// #419: Claude Code lists the bundle's `torlek` skill because the launch adds the folder
/// whose `.claude/skills` holds it, written at the daemon's start.
#[tokio::test(flavor = "multi_thread")]
async fn a_claude_launch_adds_the_instruction_skills() {
    let home = Home::new();
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, "app").await;
    let session = new_session(&client, &root).await;
    home.wait_for_launches(&session.id, 1).await;
    let skills_root = home.state_dir().join("claude-skills");

    let arguments = home.arguments_of(&session.id);

    assert!(
        arguments[0].contains(&format!("--add-dir {}", skills_root.display())),
        "{arguments:?}"
    );
    let skill =
        std::fs::read_to_string(skills_root.join(".claude/skills/torlek/SKILL.md")).unwrap();
    assert!(skill.starts_with("---\nname: torlek\n"), "{skill}");
}

/// #419: a sandboxed Claude Code runs the launch's `SessionStart` hook in the sandbox, and
/// its answer still adds this session's brief to the model's context.
#[tokio::test(flavor = "multi_thread")]
async fn a_sandboxed_claude_gets_the_sessions_brief_as_its_context_starts() {
    let home = Home::new();
    let agent = sandboxed_agent(&home, |_| {}).await;

    let printed = home.start_claude_context(&agent.root).await;

    let context = added_context(&printed);
    assert!(
        context.contains(&format!("Session {}", agent.session)),
        "{context}"
    );
}

/// #419: the MCP server a Claude launch registers runs in the sandbox and reaches the daemon.
#[tokio::test(flavor = "multi_thread")]
async fn a_sandboxed_claudes_mcp_server_answers_the_sessions_brief() {
    let home = Home::new();
    let agent = sandboxed_agent(&home, |_| {}).await;

    let printed = home.call_claude_mcp_tool(&agent.root, "torlek_brief").await;

    let brief = mcp_tool_text(&printed);
    assert!(
        brief.contains(&format!("Session {}", agent.session)),
        "{brief}"
    );
}

/// #419: the skill a sandboxed Claude Code is pointed at with `--add-dir` is readable there.
#[tokio::test(flavor = "multi_thread")]
async fn a_sandboxed_claude_reads_the_skill_its_launch_adds() {
    let home = Home::new();
    let agent = sandboxed_agent(&home, |_| {}).await;
    let arguments = home.launch_arguments(&agent.root).await;
    let added = launch_option(&arguments, "--add-dir");

    let printed = run_in_agent(
        &home,
        &agent.root,
        &format!("head -n 2 '{added}/.claude/skills/torlek/SKILL.md'"),
    )
    .await;

    assert_eq!(printed, "---\nname: torlek\nexit=0\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_may_not_read_a_sessions_sandbox_or_the_profile() {
    let home = Home::new();
    let _daemon = home.start();
    let agent = Connection::open(&home.agent_socket_path(), GENEROUS)
        .await
        .unwrap();

    for command in [
        Command::SessionSandbox {
            session: "s1".to_owned(),
        },
        Command::SandboxShow {
            dir: home.path().display().to_string(),
            agent: None,
        },
    ] {
        let refused = agent.request(command, GENEROUS).await;

        assert!(
            matches!(&refused, Err(ClientError::Daemon(error)) if error.code == ErrorCode::NotAuthorized),
            "{refused:?}"
        );
    }
}

/// The checkout of the [`COMMANDED_PROJECT`] agent, before the daemon registers it.
fn commanded_checkout(home: &Home) -> PathBuf {
    let root = home.path().join("projects").join(COMMANDED_PROJECT);
    std::fs::create_dir_all(&root).unwrap();
    root
}

/// Writes an executable shell script at `path`.
fn write_program(path: &Path, script: &str) {
    use std::os::unix::fs::PermissionsExt;

    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, script).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Every file under `folder`, with its contents, in a fixed order.
fn snapshot(folder: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut files = Vec::new();
    let mut pending = vec![folder.to_path_buf()];
    while let Some(current) = pending.pop() {
        for entry in std::fs::read_dir(&current).unwrap().flatten() {
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                pending.push(path);
            } else {
                files.push((path.clone(), std::fs::read(&path).unwrap()));
            }
        }
    }
    files.sort();
    files
}

/// A Rust checkout whose owner installed a toolchain in `~/.cargo/bin`, with a registry
/// token beside it, rustup's toolchains in `~/.rustup` and their own scripts in
/// `~/.local/bin`, all on the agent's `PATH`. The `cargo` there stands in for a build: it
/// fetches into `CARGO_HOME` and prints where that is.
async fn rust_agent(home: &Home) -> Sandboxed {
    let cargo_bin = home.path().join(".cargo/bin");
    let local_bin = home.path().join(".local/bin");
    sandboxed_agent_on_path(home, &[cargo_bin.clone(), local_bin.clone()], |home| {
        std::fs::write(
            commanded_checkout(home).join("Cargo.toml"),
            "[package]\nname = \"fixture\"\n",
        )
        .unwrap();
        write_program(
            &cargo_bin.join("cargo"),
            "#!/bin/sh\nmkdir -p \"$CARGO_HOME/registry\" && echo fetched > \"$CARGO_HOME/registry/crate\" && echo \"cargo home: $CARGO_HOME\"\n",
        );
        std::fs::write(home.path().join(".cargo/credentials.toml"), "token = \"owner's\"\n").unwrap();
        std::fs::create_dir_all(home.path().join(".cargo/registry")).unwrap();
        std::fs::write(home.path().join(".cargo/registry/owners-crate"), "owner's\n").unwrap();
        std::fs::create_dir_all(home.path().join(".rustup/toolchains")).unwrap();
        std::fs::write(home.path().join(".rustup/settings.toml"), "default_toolchain = \"stable\"\n").unwrap();
        write_program(&local_bin.join("owner-tool"), "#!/bin/sh\necho owner's tool\n");
    })
    .await
}

/// #859: the toolchain table binds the install root `PATH` resolves the project's tools in,
/// read-only, with what those tools need beside it.
#[tokio::test(flavor = "multi_thread")]
async fn a_toolchain_on_the_path_runs_from_a_read_only_install_root() {
    let home = Home::new();
    let agent = rust_agent(&home).await;

    let printed = run_in_agent(
        &home,
        &agent.root,
        "cargo >/dev/null && echo cargo-ran; cat ~/.rustup/settings.toml; \
         touch ~/.cargo/bin/planted || echo bin-refused",
    )
    .await;

    assert!(printed.contains("cargo-ran\n"), "{printed}");
    assert!(printed.contains("default_toolchain"), "{printed}");
    assert!(printed.contains("bin-refused\n"), "{printed}");
    assert!(!home.path().join(".cargo/bin/planted").exists());
}

/// #859: a token beside a toolchain root, and a `PATH` folder outside the table, are not
/// mounted.
#[tokio::test(flavor = "multi_thread")]
async fn nothing_beside_a_toolchain_root_nor_outside_the_table_is_visible() {
    let home = Home::new();
    let agent = rust_agent(&home).await;

    for unreadable in [
        home.path().join(".cargo/credentials.toml"),
        home.path().join(".local/bin/owner-tool"),
    ] {
        let printed = run_in_agent(
            &home,
            &agent.root,
            &format!("cat '{}' || echo refused", unreadable.display()),
        )
        .await;

        assert!(
            printed.ends_with("refused\nexit=0\n"),
            "{unreadable:?}: {printed}"
        );
    }
}

/// #859 Q1 and Q3: the owner's tool configuration and caches are not mounted.
#[tokio::test(flavor = "multi_thread")]
async fn a_sandboxed_agent_sees_none_of_the_owners_tool_configuration_or_caches() {
    let home = Home::new();
    let agent = sandboxed_agent(&home, |home| {
        for (file, contents) in [
            (".config/tool/hosts.yml", "token: owner's\n"),
            (".cache/sccache/object", "compiled\n"),
            (".gitconfig", "[credential]\n\thelper = store\n"),
        ] {
            let path = home.path().join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, contents).unwrap();
        }
    })
    .await;

    for unreadable in [
        ".config/tool/hosts.yml",
        ".cache/sccache/object",
        ".gitconfig",
    ] {
        let printed = run_in_agent(
            &home,
            &agent.root,
            &format!("cat ~/'{unreadable}' || echo refused"),
        )
        .await;

        assert!(
            printed.ends_with("refused\nexit=0\n"),
            "{unreadable}: {printed}"
        );
    }
}

/// #859 Q1: a build fetches into the session's own cache; the owner's stays as it was.
#[tokio::test(flavor = "multi_thread")]
async fn a_build_fetches_into_the_sessions_own_cache_and_leaves_the_owners_unchanged() {
    let home = Home::new();
    let owners_cargo = home.path().join(".cargo");
    let agent = rust_agent(&home).await;
    let before = snapshot(&owners_cargo);

    let printed = run_in_agent(&home, &agent.root, "cargo").await;

    let session_cache = home.session_temp_dir(&agent.session).join("caches/cargo");
    assert!(
        printed.contains(&format!("cargo home: {}\n", session_cache.display())),
        "{printed}"
    );
    assert_eq!(
        std::fs::read_to_string(session_cache.join("registry/crate")).unwrap(),
        "fetched\n"
    );
    assert_eq!(snapshot(&owners_cargo), before);
}

/// #859 Q1: one session's cache is its own; another session cannot read it.
#[tokio::test(flavor = "multi_thread")]
async fn a_second_session_does_not_see_the_first_ones_cache() {
    let home = Home::new();
    let first = sandboxed_agent(&home, |_| {}).await;
    let printed = run_in_agent(
        &home,
        &first.root,
        "mkdir -p \"$CARGO_HOME\" && echo first > \"$CARGO_HOME/marker\" && echo written",
    )
    .await;
    assert!(printed.contains("written\n"), "{printed}");
    let marker = home
        .session_temp_dir(&first.session)
        .join("caches/cargo/marker");
    assert!(marker.is_file());
    let client = home.connect().await;
    let (second_root, _) = home
        .project_at(
            &client,
            &format!("second/{COMMANDED_PROJECT}"),
            Some("second"),
        )
        .await;
    let _second = new_session(&client, &second_root).await;

    let printed = run_in_agent(
        &home,
        &second_root,
        &format!("cat '{}' || echo refused", marker.display()),
    )
    .await;

    assert!(printed.ends_with("refused\nexit=0\n"), "{printed}");
}

/// #859: Git works in the checkout from the configuration Torlek writes, with the owner's
/// identity for that checkout copied as values: an `includeIf` on its folder applies.
#[tokio::test(flavor = "multi_thread")]
async fn git_commits_in_the_checkout_with_the_owners_identity() {
    let home = Home::new();
    let agent = sandboxed_agent(&home, |home| {
        let projects = home.path().join("projects");
        std::fs::write(
            home.path().join(".gitconfig"),
            format!(
                "[user]\n\tname = Owner Fixture\n\temail = owner@personal.example\n\
                 [includeIf \"gitdir:{}/\"]\n\tpath = {}\n",
                projects.display(),
                home.path().join("work.gitconfig").display()
            ),
        )
        .unwrap();
        std::fs::write(
            home.path().join("work.gitconfig"),
            "[user]\n\temail = owner@example.com\n",
        )
        .unwrap();
    })
    .await;

    let printed = run_in_agent(
        &home,
        &agent.root,
        "git commit --quiet --allow-empty -m sandboxed && git log -1 --format='%an <%ae> %s'",
    )
    .await;

    assert!(
        printed.ends_with("Owner Fixture <owner@example.com> sandboxed\nexit=0\n"),
        "{printed}"
    );
}

/// #859: the session's end removes its Go module cache, even as Go leaves it without
/// `-modcacherw`: read-only folders.
#[tokio::test(flavor = "multi_thread")]
async fn ending_the_session_removes_its_populated_go_module_cache() {
    let home = Home::new();
    let agent = sandboxed_agent(&home, |home| {
        std::fs::write(
            commanded_checkout(home).join("go.mod"),
            "module example.com/fixture\n",
        )
        .unwrap();
    })
    .await;
    let printed = run_in_agent(
        &home,
        &agent.root,
        "module=\"$GOMODCACHE/example.com/dependency@v1.0.0\" && mkdir -p \"$module\" && \
         echo 'module example.com/dependency' > \"$module/go.mod\" && \
         chmod -R a-w \"$GOMODCACHE/example.com\" && echo \"flags: $GOFLAGS\"",
    )
    .await;
    assert!(printed.contains("flags: -modcacherw\n"), "{printed}");
    let temp_dir = home.session_temp_dir(&agent.session);
    assert!(temp_dir.join("caches/go-mod/example.com").is_dir());
    let client = home.connect().await;

    client
        .request(
            Command::SessionEnd {
                session: agent.session.clone(),
            },
            GENEROUS,
        )
        .await
        .expect("the owner ends the session");

    let deadline = tokio::time::Instant::now() + GENEROUS;
    while temp_dir.exists() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(!temp_dir.exists(), "{}", temp_dir.display());
}

/// #859: the evidence names the cache mode and the toolchains it detected.
#[tokio::test(flavor = "multi_thread")]
async fn the_evidence_names_the_session_own_cache_mode_and_the_detected_toolchains() {
    let home = Home::new();
    let agent = rust_agent(&home).await;

    let shown = home
        .cockpit(&["session", "sandbox", &agent.session], &[])
        .await;

    assert!(shown.status.success(), "{}", text(&shown.stderr));
    let printed = text(&shown.stdout);
    assert!(
        printed.contains("toolchains detected in the checkout: rust"),
        "{printed}"
    );
    assert!(printed.contains("cache mode: session-own"), "{printed}");
    assert!(
        printed.contains(&format!(
            "read-only {} when present (toolchain install root)",
            home.path().join(".cargo/bin").display()
        )),
        "{printed}"
    );
}

/// #859: the session's end removes its tool configuration, which holds the owner's Git
/// identity.
#[tokio::test(flavor = "multi_thread")]
async fn ending_the_session_removes_its_git_configuration() {
    let home = Home::new();
    let agent = sandboxed_agent(&home, |_| {}).await;
    let printed = run_in_agent(
        &home,
        &agent.root,
        "echo \"git config: $GIT_CONFIG_GLOBAL\"",
    )
    .await;
    let configuration = PathBuf::from(
        printed
            .lines()
            .find_map(|line| line.strip_prefix("git config: "))
            .unwrap_or_else(|| panic!("{printed}")),
    );
    assert!(configuration.is_file(), "{}", configuration.display());
    let client = home.connect().await;

    client
        .request(
            Command::SessionEnd {
                session: agent.session.clone(),
            },
            GENEROUS,
        )
        .await
        .expect("the owner ends the session");

    let deadline = tokio::time::Instant::now() + GENEROUS;
    while configuration.exists() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(!configuration.exists(), "{}", configuration.display());
}

/// #482: the session's end removes its folder of the daemon's mount sources.
#[tokio::test(flavor = "multi_thread")]
async fn ending_the_session_removes_its_mount_source_folder() {
    let home = Home::new();
    let agent = sandboxed_agent(&home, |_| {}).await;
    run_in_agent(&home, &agent.root, "true").await;
    let mount_sources = home
        .path()
        .join("run")
        .join(BUILD_PROFILE_DIR)
        .join("mounts")
        .join(&agent.session);
    assert!(mount_sources.is_dir(), "{}", mount_sources.display());
    let client = home.connect().await;

    client
        .request(
            Command::SessionEnd {
                session: agent.session.clone(),
            },
            GENEROUS,
        )
        .await
        .expect("the owner ends the session");

    let deadline = tokio::time::Instant::now() + GENEROUS;
    while mount_sources.exists() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(!mount_sources.exists(), "{}", mount_sources.display());
}

/// A raw `CONNECT target` to the session proxy from inside, with bash's `/dev/tcp`: the
/// proxy's whole answer, then how many milliseconds it took.
fn connect_line(target: &str) -> String {
    format!(
        "start=$(date +%s%N); bash -c 'exec 3<>/dev/tcp/127.0.0.1/3128; \
         printf \"CONNECT {target} HTTP/1.1\\r\\nHost: {target}\\r\\n\\r\\n\" >&3; cat <&3'; \
         echo \"ms=$(( ($(date +%s%N) - start) / 1000000 ))\""
    )
}

fn milliseconds(printed: &str) -> u64 {
    printed
        .lines()
        .find_map(|line| line.strip_prefix("ms="))
        .and_then(|ms| ms.parse().ok())
        .unwrap_or_else(|| panic!("no time in {printed}"))
}

/// #831: an unknown host gets `403` in under a second, naming the host, the session and
/// why, and the client inside reads that the proxy, not the network, refused it.
#[tokio::test(flavor = "multi_thread")]
async fn a_sandboxed_agent_is_refused_an_unknown_host_at_once_with_a_reason() {
    let home = Home::new();
    let agent = sandboxed_agent(&home, |_| {}).await;

    let printed = run_in_agent(&home, &agent.root, &connect_line("unknown.example.com:443")).await;

    assert!(printed.starts_with("HTTP/1.1 403 Forbidden: "), "{printed}");
    assert!(printed.contains("error=http_request_denied"), "{printed}");
    assert!(
        printed.contains(&format!(
            "torlek-sandbox refused unknown.example.com:443 for session {}: \
             unknown.example.com:443 is not allowed",
            agent.session
        )),
        "{printed}"
    );
    assert!(milliseconds(&printed) < 1000, "{printed}");
}

/// #831: loopback, private, link-local (metadata) addresses and IP literals are refused:
/// the host's own services are not reachable from inside.
#[tokio::test(flavor = "multi_thread")]
async fn a_sandboxed_agent_is_refused_ip_literals_loopback_private_and_metadata_addresses() {
    let home = Home::new();
    let agent = sandboxed_agent(&home, |_| {}).await;

    for target in [
        "127.0.0.1:22",
        "[::1]:22",
        "10.0.0.1:443",
        "169.254.169.254:80",
        "1.1.1.1:443",
    ] {
        let printed = run_in_agent(&home, &agent.root, &connect_line(target)).await;

        assert!(
            printed.starts_with("HTTP/1.1 403 Forbidden: "),
            "{target}: {printed}"
        );
        assert!(
            printed.contains("error=destination_ip_prohibited"),
            "{target}: {printed}"
        );
    }
}

/// #831: no resolver answers inside and no UDP leaves: no DNS tunnel.
#[tokio::test(flavor = "multi_thread")]
async fn a_sandboxed_agent_has_no_dns_and_no_udp() {
    let home = Home::new();
    let agent = sandboxed_agent(&home, |_| {}).await;

    let printed = run_in_agent(
        &home,
        &agent.root,
        "getent hosts example.com || echo unresolved; \
         bash -c 'echo probe > /dev/udp/1.1.1.1/53' 2>/dev/null || echo 'no udp'",
    )
    .await;

    assert_eq!(printed, "unresolved\nno udp\nexit=0\n");
}

/// #831: clients inside find the proxy through the usual variables.
#[tokio::test(flavor = "multi_thread")]
async fn a_sandboxed_agents_clients_are_pointed_at_the_session_proxy() {
    let home = Home::new();
    let agent = sandboxed_agent(&home, |_| {}).await;

    let printed = run_in_agent(&home, &agent.root, "echo \"$HTTPS_PROXY $NO_PROXY\"").await;

    assert_eq!(
        printed,
        "http://127.0.0.1:3128 localhost,127.0.0.1,::1\nexit=0\n"
    );
}

/// #831, #861: the evidence names each allowed host with its source, and the counters the
/// launcher keeps show what it refused.
#[tokio::test(flavor = "multi_thread")]
async fn the_evidence_names_the_allowed_hosts_and_counts_refusals() {
    let home = Home::new();
    let agent = sandboxed_agent(&home, |_| {}).await;
    run_in_agent(&home, &agent.root, &connect_line("unknown.example.com:443")).await;

    let printed = sandbox_evidence_once_it_contains(
        &home,
        &agent.session,
        "egress unknown.example.com:443: refused 1",
    )
    .await;

    assert!(
        printed.contains(
            "egress: api.anthropic.com:443 (the claude adapter), TLS server name checked"
        ),
        "{printed}"
    );
    assert!(
        printed.contains(
            "egress unknown.example.com:443: allowed 0, refused 1, unreachable 0, \
             0 bytes out, 0 bytes in; last: unknown.example.com:443 is not allowed"
        ),
        "{printed}"
    );
}

/// #859: a checkout that uses an ecosystem gets its registry, named as such.
#[tokio::test(flavor = "multi_thread")]
async fn a_checkouts_ecosystem_registry_is_allowed_and_named() {
    let home = Home::new();
    home.write_owner_config(REQUIRED);
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, COMMANDED_PROJECT).await;
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"app\"\n").unwrap();
    let session = new_session(&client, &root).await;

    let shown = home
        .cockpit(&["session", "sandbox", &session.id], &[])
        .await;

    assert!(shown.status.success(), "{}", text(&shown.stderr));
    let printed = text(&shown.stdout);
    for host in ["index.crates.io", "static.crates.io"] {
        assert!(
            printed.contains(&format!(
                "egress: {host}:443 (the rust registry), TLS server name checked"
            )),
            "{printed}"
        );
    }
}

/// `cockpit session sandbox` once it prints `expected`: the launcher rewrites its counters
/// within a second of a change.
async fn sandbox_evidence_once_it_contains(home: &Home, session: &str, expected: &str) -> String {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let shown = home.cockpit(&["session", "sandbox", session], &[]).await;
        assert!(shown.status.success(), "{}", text(&shown.stderr));
        let printed = text(&shown.stdout);
        if printed.contains(expected) || std::time::Instant::now() > deadline {
            return printed;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// #674: a host no entry allows, under a reserved name no resolver sends anywhere (RFC 6761).
const ASKED_HOST: &str = "unknown.invalid:443";

/// `cockpit network list` once it prints `expected`: the daemon reads the launcher's
/// requests file every quarter second.
async fn network_list_once_it_contains(home: &Home, expected: &str) -> String {
    let deadline = std::time::Instant::now() + GENEROUS;
    loop {
        let listed = home.cockpit(&["network", "list"], &[]).await;
        assert!(listed.status.success(), "{}", text(&listed.stderr));
        let printed = text(&listed.stdout);
        if printed.contains(expected) || std::time::Instant::now() > deadline {
            return printed;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// The `cockpit network list` line of the first request, asking for [`ASKED_HOST`].
fn asking_line(session: &str) -> String {
    asking_line_for(session, ASKED_HOST)
}

/// The `cockpit network list` line of the first request, asking for `destination`.
fn asking_line_for(session: &str, destination: &str) -> String {
    format!("network\t1\tasking\t{session}\t{COMMANDED_PROJECT}\t{destination}")
}

/// Refuses [`ASKED_HOST`] once and waits until the owner's list shows the request, then out
/// of the grace in which an allow is refused (#156).
async fn asked_and_listed(home: &Home, agent: &Sandboxed) {
    asked_and_listed_for(home, agent, ASKED_HOST).await;
}

/// [`asked_and_listed`] for `destination`.
async fn asked_and_listed_for(home: &Home, agent: &Sandboxed, destination: &str) {
    run_in_agent(home, &agent.root, &connect_line(destination)).await;
    let asking = asking_line_for(&agent.session, destination);
    let listed = network_list_once_it_contains(home, &asking).await;
    assert!(listed.contains(&asking), "{listed}");
    tokio::time::sleep(Duration::from_millis(2_100)).await;
}

/// #832, #674: a GitHub host the owner allowed always for the project is reached at the
/// project's next launch, so the API token is bound there: the agent runs with the placeholder
/// in `GH_TOKEN`, the launcher took the value's file, and the evidence says where the token is
/// added and names the host still unreached.
#[tokio::test(flavor = "multi_thread")]
async fn an_api_token_is_added_on_a_github_host_the_owner_allowed_always() {
    let home = Home::new();
    let agent = sandboxed_agent(&home, |home| {
        home.write_github_api_token(COMMANDED_PROJECT, "github_pat_api_token\n", 0o600);
    })
    .await;
    asked_and_listed_for(&home, &agent, "api.github.com:443").await;
    owner_network(&home, &["allow", "1", "--always"]).await;
    let client = home.connect().await;
    let ended = client
        .request(
            Command::SessionEnd {
                session: agent.session.clone(),
            },
            GENEROUS,
        )
        .await;
    assert!(ended.is_ok(), "{ended:?}");
    std::fs::remove_dir_all(agent.root.join(".agent")).unwrap();
    let session = new_session(&client, &agent.root).await;

    let printed = run_in_agent(&home, &agent.root, "echo \"${GH_TOKEN-unset}\"").await;
    let shown = home
        .cockpit(&["session", "sandbox", &session.id], &[])
        .await;
    let credentials = home
        .path()
        .join("run")
        .join(BUILD_PROFILE_DIR)
        .join("mounts")
        .join(&session.id)
        .join(cockpit_sandbox::CREDENTIALS_FILE);

    assert_eq!(
        printed,
        format!("{}\nexit=0\n", cockpit_sandbox::PLACEHOLDER)
    );
    assert!(!credentials.exists(), "{credentials:?}");
    let evidence = text(&shown.stdout);
    assert!(
        evidence.contains("credential: the project's GitHub API token added on api.github.com:443"),
        "{evidence}"
    );
    assert!(
        evidence.contains(
            "credential: the project's GitHub API token is not added on github.com:443: the \
             session's launch does not reach it"
        ),
        "{evidence}"
    );
}

/// #1256: the github.com grant the owner makes while adding the project, before any session
/// asked, is reached at the project's first launch, so the API token is bound there with no
/// request and no relaunch.
#[tokio::test(flavor = "multi_thread")]
async fn a_github_grant_made_before_the_first_launch_binds_the_token_on_github_at_once() {
    let home = Home::new();
    home.write_owner_config(REQUIRED);
    home.write_github_api_token(COMMANDED_PROJECT, "github_pat_api_token\n", 0o600);
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, COMMANDED_PROJECT).await;
    owner_network(&home, &["grant", COMMANDED_PROJECT, "github.com"]).await;

    let session = new_session(&client, &root).await;

    let printed = run_in_agent(&home, &root, "echo \"${GH_TOKEN-unset}\"").await;
    let shown = home
        .cockpit(&["session", "sandbox", &session.id], &[])
        .await;
    assert_eq!(
        printed,
        format!("{}\nexit=0\n", cockpit_sandbox::PLACEHOLDER)
    );
    let evidence = text(&shown.stdout);
    assert!(
        evidence.contains("credential: the project's GitHub API token added on github.com:443"),
        "{evidence}"
    );
    assert_eq!(owner_network(&home, &["list"]).await, "");
}

/// A `claude setup-token` token's shape; a fixture, never a real one.
const SETUP_TOKEN: &str = "sk-ant-oat01-fixture_value";

/// #968, #858: the owner's Claude credential reaches a sandboxed session through the proxy:
/// the agent holds the placeholder in the variable the file is named after, the launcher took
/// the value's file, and the evidence says where it is added.
#[tokio::test(flavor = "multi_thread")]
async fn the_providers_credential_is_added_on_its_hosts_and_the_agent_holds_a_placeholder() {
    let home = Home::new();
    let agent = sandboxed_agent(&home, |home| {
        home.write_provider_credential(
            "claude",
            "CLAUDE_CODE_OAUTH_TOKEN",
            &format!("{SETUP_TOKEN}\n"),
            0o600,
        );
    })
    .await;

    let printed = run_in_agent(
        &home,
        &agent.root,
        "echo \"${CLAUDE_CODE_OAUTH_TOKEN-unset}\"; env | grep -c fixture_value",
    )
    .await;
    let shown = home
        .cockpit(&["session", "sandbox", &agent.session], &[])
        .await;
    let credentials = home
        .path()
        .join("run")
        .join(BUILD_PROFILE_DIR)
        .join("mounts")
        .join(&agent.session)
        .join(cockpit_sandbox::CREDENTIALS_FILE);

    assert_eq!(
        printed,
        format!("{}\n0\nexit=1\n", cockpit_sandbox::PLACEHOLDER)
    );
    assert!(!credentials.exists(), "{credentials:?}");
    let evidence = text(&shown.stdout);
    assert!(
        evidence.contains(
            "credential: the claude provider's credential of account default \
             (CLAUDE_CODE_OAUTH_TOKEN) added on api.anthropic.com:443"
        ),
        "{evidence}"
    );
}

/// The owner's accounts for Claude, in the order a launch tries them (#59).
const TWO_ACCOUNTS: &str = "[accounts]\nclaude = [\"work\", \"spare\"]\n";

/// What `session sandbox` prints for `session`.
async fn session_sandbox(home: &Home, session: &str) -> String {
    text(
        &home
            .cockpit(&["session", "sandbox", session], &[])
            .await
            .stdout,
    )
}

/// The account `session`'s latest launch runs on, as the daemon lists it.
async fn launch_account(home: &Home, session: &str) -> Option<String> {
    let client = home.connect().await;
    listed(&client, session)
        .await
        .launch
        .expect("the session launched")
        .account
}

/// #59: a sandboxed launch runs on the first of the owner's accounts that holds a credential;
/// the session proxy adds that account's credential, and the daemon records the account.
#[tokio::test(flavor = "multi_thread")]
async fn a_sandboxed_launch_runs_on_the_first_listed_account_with_a_credential() {
    let home = Home::new();
    let agent = sandboxed_agent(&home, |home| {
        home.write_owner_config(&format!("{REQUIRED}{TWO_ACCOUNTS}"));
        for account in ["work", "spare"] {
            home.write_account_credential(
                "claude",
                account,
                "CLAUDE_CODE_OAUTH_TOKEN",
                &format!("{SETUP_TOKEN}_{account}\n"),
                0o600,
            );
        }
    })
    .await;

    let printed = run_in_agent(
        &home,
        &agent.root,
        "echo \"${CLAUDE_CODE_OAUTH_TOKEN-unset}\"",
    )
    .await;
    let evidence = session_sandbox(&home, &agent.session).await;
    let account = launch_account(&home, &agent.session).await;

    assert_eq!(
        printed,
        format!("{}\nexit=0\n", cockpit_sandbox::PLACEHOLDER)
    );
    assert!(
        evidence.contains(
            "credential: the claude provider's credential of account work \
             (CLAUDE_CODE_OAUTH_TOKEN) added on api.anthropic.com:443"
        ),
        "{evidence}"
    );
    assert_eq!(account.as_deref(), Some("work"));
}

/// #59: an account without a credential is passed over for the next one in the owner's order.
#[tokio::test(flavor = "multi_thread")]
async fn an_account_without_a_credential_is_passed_over_for_the_next() {
    let home = Home::new();
    let agent = sandboxed_agent(&home, |home| {
        home.write_owner_config(&format!("{REQUIRED}{TWO_ACCOUNTS}"));
        home.write_account_credential(
            "claude",
            "spare",
            "ANTHROPIC_API_KEY",
            &format!("{SETUP_TOKEN}\n"),
            0o600,
        );
    })
    .await;

    run_in_agent(&home, &agent.root, "true").await;
    let evidence = session_sandbox(&home, &agent.session).await;
    let account = launch_account(&home, &agent.session).await;

    assert!(
        evidence.contains("the claude provider's credential of account spare (ANTHROPIC_API_KEY)"),
        "{evidence}"
    );
    assert_eq!(account.as_deref(), Some("spare"));
}

/// A session launched on `spare` alone, `work` holding no credential yet.
async fn agent_launched_on_spare(home: &Home) -> Sandboxed {
    let agent = sandboxed_agent(home, |home| {
        home.write_owner_config(&format!("{REQUIRED}{TWO_ACCOUNTS}"));
        home.write_account_credential(
            "claude",
            "spare",
            "ANTHROPIC_API_KEY",
            &format!("{SETUP_TOKEN}\n"),
            0o600,
        );
    })
    .await;
    run_in_agent(home, &agent.root, "true").await;
    assert_eq!(
        launch_account(home, &agent.session).await.as_deref(),
        Some("spare")
    );
    agent
}

async fn restart(home: &Home, session: &str) -> Result<(), ClientError> {
    home.connect()
        .await
        .request(
            Command::SessionRestart {
                session: session.to_owned(),
            },
            GENEROUS,
        )
        .await
        .map(drop)
}

/// #59, report 18: a restart keeps the account its conversation runs on, even when an earlier
/// listed account now holds a credential; only a first launch picks.
#[tokio::test(flavor = "multi_thread")]
async fn a_restart_keeps_the_account_its_conversation_runs_on() {
    let home = Home::new();
    let agent = agent_launched_on_spare(&home).await;
    home.write_account_credential(
        "claude",
        "work",
        "ANTHROPIC_API_KEY",
        &format!("{SETUP_TOKEN}\n"),
        0o600,
    );

    restart(&home, &agent.session).await.unwrap();

    let client = home.connect().await;
    let launch = listed(&client, &agent.session)
        .await
        .launch
        .expect("the restart launched");
    assert_eq!(launch.kind, LaunchKindView::Restart);
    assert_eq!(launch.account.as_deref(), Some("spare"));
}

/// #59: a restart whose account lost its credential is refused, naming the account, rather
/// than moved to another account.
#[tokio::test(flavor = "multi_thread")]
async fn a_restart_whose_account_lost_its_credential_is_refused_naming_it() {
    let home = Home::new();
    let agent = agent_launched_on_spare(&home).await;
    home.write_account_credential(
        "claude",
        "work",
        "ANTHROPIC_API_KEY",
        &format!("{SETUP_TOKEN}\n"),
        0o600,
    );
    std::fs::remove_file(
        home.path()
            .join("config")
            .join(BUILD_PROFILE_DIR)
            .join(cockpit_config::PROVIDER_CREDENTIALS_DIR)
            .join("claude/spare/ANTHROPIC_API_KEY"),
    )
    .unwrap();

    let restarted = restart(&home, &agent.session).await;

    assert!(
        matches!(&restarted, Err(ClientError::Daemon(error))
            if error.code == ErrorCode::Refused
                && error.message.contains("account spare, which holds no credential or login any more")),
        "{restarted:?}"
    );
}

/// A sandboxed session on `work`, `spare` holding a credential too.
async fn agent_on_work_with_spare(home: &Home) -> Sandboxed {
    let agent = sandboxed_agent(home, |home| {
        home.write_owner_config(&format!("{REQUIRED}{TWO_ACCOUNTS}"));
        for name in ["work", "spare"] {
            home.write_account_credential(
                "claude",
                name,
                "ANTHROPIC_API_KEY",
                &format!("{SETUP_TOKEN}\n"),
                0o600,
            );
        }
    })
    .await;
    run_in_agent(home, &agent.root, "true").await;
    agent
}

/// The line the agent runs to report that the usage limit stopped its turn, as Codex's
/// launcher does.
fn quota_stop_line() -> String {
    format!(
        "'{}' agent report --state waiting --outcome quota-stopped",
        cockpit_program().display()
    )
}

/// #59 acceptance, report 18: a quota stop the session reports relaunches it on the next
/// listed account within one minute, resuming the same conversation, and the launch records
/// the account it moved off.
#[tokio::test(flavor = "multi_thread")]
async fn a_quota_stop_resumes_the_conversation_on_the_next_account_within_a_minute() {
    let home = Home::new();
    let agent = agent_on_work_with_spare(&home).await;
    let client = home.connect().await;
    let before = listed(&client, &agent.session).await;
    assert_eq!(
        before.launch.as_ref().unwrap().account.as_deref(),
        Some("work")
    );

    std::fs::remove_file(agent.root.join(".agent/launch.arguments")).unwrap();
    let reported = Instant::now();
    home.command_agent(&agent.root, &quota_stop_line());
    let after = loop {
        let session = listed(&client, &agent.session).await;
        if session.launch.as_ref().unwrap().account.as_deref() == Some("spare")
            && session.state == SessionStateView::Running
        {
            break session;
        }
        // The acceptance asks for a minute; half a 30-second pass proves the report itself
        // starts the relaunch, the pass being only its fallback.
        assert!(
            reported.elapsed() < Duration::from_secs(15),
            "not relaunched on spare within 15 seconds: {session:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };

    let launch = after.launch.unwrap();
    assert_eq!(launch.kind, LaunchKindView::Restart);
    assert_eq!(launch.switched_from.as_deref(), Some("work"));
    assert!(before.conversation.is_some());
    assert_eq!(after.conversation, before.conversation);
    let arguments = home.launch_arguments(&agent.root).await.join(" ");
    assert!(
        arguments.contains(&format!("--resume {}", before.conversation.unwrap())),
        "{arguments}"
    );
    // #158: the move makes the stop due its continue message without waiting for a reset.
    let switched = Instant::now();
    loop {
        let stop = listed(&client, &agent.session).await.quota_stop;
        if stop.as_ref().is_some_and(|stop| stop.switched_at.is_some()) {
            break;
        }
        assert!(
            switched.elapsed() < Duration::from_secs(5),
            "the quota stop does not record the move: {stop:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// #59: after an account quota-stops, a new session starts on the next account with
/// capacity rather than on it.
#[tokio::test(flavor = "multi_thread")]
async fn a_new_session_passes_over_an_account_that_quota_stopped() {
    let home = Home::new();
    let agent = agent_on_work_with_spare(&home).await;
    let client = home.connect().await;
    home.command_agent(&agent.root, &quota_stop_line());
    wait_for_launch_account(&home, &agent.session, "spare").await;

    let (other, _) = home.project(&client, "other").await;
    let started = new_session(&client, &other).await;

    wait_for_launch_account(&home, &started.id, "spare").await;
}

/// Waits until `session`'s latest launch runs on `wanted`.
async fn wait_for_launch_account(home: &Home, session: &str, wanted: &str) {
    let client = home.connect().await;
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let launch = listed(&client, session).await.launch;
        if launch.as_ref().and_then(|launch| launch.account.as_deref()) == Some(wanted) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{session} never ran on {wanted}: {launch:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// #59: with no other account holding a credential, a quota stop leaves the session on its
/// account and running.
#[tokio::test(flavor = "multi_thread")]
async fn a_quota_stop_with_no_other_account_keeps_the_session_where_it_is() {
    let home = Home::new();
    let agent = sandboxed_agent(&home, |home| {
        home.write_owner_config(&format!("{REQUIRED}{TWO_ACCOUNTS}"));
        home.write_account_credential(
            "claude",
            "work",
            "ANTHROPIC_API_KEY",
            &format!("{SETUP_TOKEN}\n"),
            0o600,
        );
    })
    .await;
    run_in_agent(&home, &agent.root, "true").await;
    let client = home.connect().await;
    let before = listed(&client, &agent.session).await.launch.unwrap();

    let reported = run_in_agent(&home, &agent.root, &quota_stop_line()).await;
    tokio::time::sleep(SETTLE).await;

    assert!(reported.ends_with("exit=0\n"), "{reported}");
    let session = listed(&client, &agent.session).await;
    assert_eq!(session.state, SessionStateView::Running);
    assert_eq!(session.launch.unwrap(), before);
}

/// #158: a quota stop whose move to another account is refused asks the owner at once, rather
/// than once the stop is overdue, and the session keeps its agent. The move is refused because
/// the owner turned the sandbox on after the session's first, unsandboxed launch (#969).
#[tokio::test(flavor = "multi_thread")]
async fn a_quota_stop_whose_account_move_is_refused_asks_the_owner_at_once() {
    let home = Home::new();
    // The accounts are read at start; only the sandbox setting follows a reload.
    home.write_owner_config(TWO_ACCOUNTS);
    for name in ["work", "spare"] {
        home.write_account_credential(
            "claude",
            name,
            "ANTHROPIC_API_KEY",
            &format!("{SETUP_TOKEN}\n"),
            0o600,
        );
    }
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, COMMANDED_PROJECT).await;
    let started = new_session(&client, &root).await;
    assert_eq!(run_in_agent(&home, &root, "true").await, "exit=0\n");
    let before = listed(&client, &started.id).await;
    assert!(before.conversation.is_some(), "{before:?}");
    home.write_owner_config(&format!("{REQUIRED}{TWO_ACCOUNTS}"));
    let reloaded = home.cockpit(&["config", "reload"], &[]).await;
    assert!(reloaded.status.success(), "{}", text(&reloaded.stderr));

    let reported = run_in_agent(&home, &root, &quota_stop_line()).await;

    assert!(reported.ends_with("exit=0\n"), "{reported}");
    let deadline = Instant::now() + Duration::from_secs(15);
    let reason = loop {
        let session = listed(&client, &started.id).await;
        if let Some(QuotaStopStatusView::Dropped { reason, .. }) =
            session.quota_stop.as_ref().map(|stop| &stop.status)
        {
            break reason.clone();
        }
        assert!(
            Instant::now() < deadline,
            "the refused move did not reach the owner: {session:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert!(
        reason.contains("its move to another account with capacity was refused"),
        "{reason}"
    );
    let reply = client
        .request(Command::AgentReports, GENEROUS)
        .await
        .unwrap();
    let Reply::AgentReports { reports } = reply else {
        panic!("{reply:?}")
    };
    let report = reports
        .iter()
        .find(|report| report.session == started.id)
        .unwrap();
    assert_eq!(report.needs_input.as_deref(), Some(reason.as_str()));
    let session = listed(&client, &started.id).await;
    assert_eq!(session.state, SessionStateView::Running);
    assert_eq!(session.launch, before.launch, "never launched again");
}

/// #59: an unusable credential refuses the launch, naming the account, rather than being
/// passed over for the next account.
#[tokio::test(flavor = "multi_thread")]
async fn an_unusable_credential_refuses_the_launch_naming_its_account() {
    let home = Home::new();
    home.write_owner_config(&format!("{REQUIRED}{TWO_ACCOUNTS}"));
    home.write_account_credential("claude", "work", "ANTHROPIC_API_KEY", SETUP_TOKEN, 0o644);
    home.write_account_credential("claude", "spare", "ANTHROPIC_API_KEY", SETUP_TOKEN, 0o600);
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, COMMANDED_PROJECT).await;

    let started = home
        .cockpit(
            &[
                "session",
                "new",
                root.to_str().unwrap(),
                "--agent",
                "claude",
            ],
            &[],
        )
        .await;

    let told = text(&started.stderr);
    assert!(!started.status.success(), "{told}");
    assert!(
        told.contains("the credential of the claude provider's account work cannot be used"),
        "{told}"
    );
    assert!(!told.contains(SETUP_TOKEN), "{told}");
}

/// The Claude login `torlek account sign-in claude NAME` keeps, where the daemon keeps it.
fn write_claude_login(home: &Home, name: &str, contents: &str) -> PathBuf {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let folder = home
        .state_dir()
        .join("account-logins/claude")
        .join(name)
        .join("login");
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&folder)
        .unwrap();
    let file = folder.join(".credentials.json");
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&file)
        .unwrap();
    std::fs::write(&file, contents).unwrap();
    file
}

fn grant(name: &str) -> [&str; 5] {
    ["account", "grant", COMMANDED_PROJECT, "claude", name]
}

/// A sandboxed [`COMMANDED_PROJECT`] agent under `accounts`, after `prepare` set up the
/// owner's home and the owner granted the project the logins `granted`.
async fn agent_with_logins(
    home: &Home,
    accounts: &str,
    granted: &[&str],
    prepare: impl FnOnce(&Home),
) -> Sandboxed {
    home.write_owner_config(&format!("{REQUIRED}{accounts}"));
    prepare(home);
    let daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, COMMANDED_PROJECT).await;
    for name in granted {
        let output = home.cockpit(&grant(name), &[]).await;
        assert!(output.status.success(), "{}", text(&output.stderr));
    }
    let session = new_session(&client, &root).await;
    Sandboxed {
        _daemon: daemon,
        root,
        session: session.id,
    }
}

const WORK_LOGIN: &str = "[accounts]\nclaude = [\"work\"]\n";

/// #59, #858: a login the project holds a grant for is the session's Claude sign-in, at the
/// path Claude Code keeps it, and the daemon records the account.
#[tokio::test(flavor = "multi_thread")]
async fn a_granted_login_is_the_sessions_claude_sign_in() {
    let home = Home::new();
    let agent = agent_with_logins(&home, WORK_LOGIN, &["work"], |home| {
        write_claude_login(home, "work", "fixture-login-v1");
    })
    .await;

    let printed = run_in_agent(&home, &agent.root, "cat ~/.claude/.credentials.json; echo").await;
    let account = launch_account(&home, &agent.session).await;

    assert_eq!(printed, "fixture-login-v1\nexit=0\n");
    assert_eq!(account.as_deref(), Some("work"));
}

/// #59: a session that launched on a granted login is told of no sign-in to write.
#[tokio::test(flavor = "multi_thread")]
async fn a_session_on_a_granted_login_needs_no_sign_in() {
    let home = Home::new();
    home.write_owner_config(&format!("{REQUIRED}{WORK_LOGIN}"));
    write_claude_login(&home, "work", "fixture-login-v1");
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, COMMANDED_PROJECT).await;
    home.cockpit(&grant("work"), &[]).await;

    let session = new_session(&client, &root).await;

    assert_eq!(session.sign_in_needed, None);
}

/// #858: Claude Code rewrites its sign-in in place when it refreshes it; the refreshed one is
/// the account's, for its next session.
#[tokio::test(flavor = "multi_thread")]
async fn a_sign_in_the_session_rewrites_in_place_reaches_the_account() {
    let home = Home::new();
    let file = home
        .state_dir()
        .join("account-logins/claude/work/login/.credentials.json");
    let agent = agent_with_logins(&home, WORK_LOGIN, &["work"], |home| {
        write_claude_login(home, "work", "fixture-login-v1");
    })
    .await;

    let printed = run_in_agent(
        &home,
        &agent.root,
        "printf fixture-login-v2 > ~/.claude/.credentials.json",
    )
    .await;

    assert_eq!(printed, "exit=0\n");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "fixture-login-v2");
    assert_eq!(
        launch_account(&home, &agent.session).await.as_deref(),
        Some("work")
    );
}

/// #59 acceptance: a refresh inside a session survives in the shared login, so the next
/// session on the account starts with the refreshed sign-in.
#[tokio::test(flavor = "multi_thread")]
async fn the_next_session_on_a_login_starts_with_the_sign_in_the_last_one_refreshed() {
    let home = Home::new();
    let agent = agent_with_logins(&home, WORK_LOGIN, &["work"], |home| {
        write_claude_login(home, "work", "fixture-login-v1");
    })
    .await;
    // The next session's agent replays every line from the first: only this one refreshes.
    let refreshed = run_in_agent(
        &home,
        &agent.root,
        &format!(
            "if [ \"$TORLEK_SESSION\" = {} ]; then printf fixture-login-v2 > \
             ~/.claude/.credentials.json; fi",
            agent.session
        ),
    )
    .await;
    assert_eq!(refreshed, "exit=0\n");
    let client = home.connect().await;
    client
        .request(
            Command::SessionEnd {
                session: agent.session.clone(),
            },
            GENEROUS,
        )
        .await
        .unwrap();

    let next = new_session(&client, &agent.root).await;
    let printed = run_in_agent(&home, &agent.root, "cat ~/.claude/.credentials.json; echo").await;

    assert_ne!(next.id, agent.session);
    assert_eq!(
        launch_account(&home, &next.id).await.as_deref(),
        Some("work")
    );
    assert!(printed.starts_with("fixture-login-v2\n"), "{printed}");
}

const PASSED_OVER_WORK: &str = "work (project commanded holds no grant for its login: \
                                `torlek account grant commanded claude work` lets it)";

/// #59, owner decision C: a login the project holds no grant for is passed over for the next
/// listed account, and the session's record names it and the grant that lets it.
#[tokio::test(flavor = "multi_thread")]
async fn an_ungranted_login_is_passed_over_for_the_next_account_and_named() {
    let home = Home::new();
    let agent = agent_with_logins(&home, TWO_ACCOUNTS, &[], |home| {
        write_claude_login(home, "work", "fixture-login-v1");
        home.write_account_credential(
            "claude",
            "spare",
            "ANTHROPIC_API_KEY",
            &format!("{SETUP_TOKEN}\n"),
            0o600,
        );
    })
    .await;

    let printed = run_in_agent(&home, &agent.root, "cat ~/.claude/.credentials.json").await;
    let client = home.connect().await;
    let launch = listed(&client, &agent.session)
        .await
        .launch
        .expect("the session launched");

    assert!(!printed.contains("fixture-login"), "{printed}");
    assert_eq!(launch.account.as_deref(), Some("spare"));
    assert_eq!(launch.passed_over, [PASSED_OVER_WORK]);
}

/// #59, owner decision C: a restart after a quota stop looks for another account, so when it
/// stays on its account it still names the login it passed over.
#[tokio::test(flavor = "multi_thread")]
async fn a_restart_after_a_quota_stop_names_the_login_it_passed_over() {
    let home = Home::new();
    let agent = agent_with_logins(&home, TWO_ACCOUNTS, &[], |home| {
        write_claude_login(home, "work", "fixture-login-v1");
        home.write_account_credential(
            "claude",
            "spare",
            "ANTHROPIC_API_KEY",
            &format!("{SETUP_TOKEN}\n"),
            0o600,
        );
    })
    .await;
    run_in_agent(&home, &agent.root, &quota_stop_line()).await;
    tokio::time::sleep(SETTLE).await;

    restart(&home, &agent.session).await.unwrap();

    let client = home.connect().await;
    let launch = listed(&client, &agent.session)
        .await
        .launch
        .expect("the session relaunched");
    assert_eq!(launch.kind, LaunchKindView::Restart);
    assert_eq!(launch.account.as_deref(), Some("spare"));
    assert_eq!(launch.passed_over, [PASSED_OVER_WORK]);
}

/// A sandboxed agent on the granted login `work`, `spare` holding a credential, and project
/// `other` granted `work` too.
async fn agent_on_work_login_with_other_project(home: &Home) -> (Sandboxed, PathBuf) {
    let agent = agent_with_logins(home, TWO_ACCOUNTS, &["work"], |home| {
        write_claude_login(home, "work", "fixture-login-v1");
        home.write_account_credential(
            "claude",
            "spare",
            "ANTHROPIC_API_KEY",
            &format!("{SETUP_TOKEN}\n"),
            0o600,
        );
    })
    .await;
    let client = home.connect().await;
    let (other, _) = home.project(&client, "other").await;
    let granted = home
        .cockpit(&["account", "grant", "other", "claude", "work"], &[])
        .await;
    assert!(granted.status.success(), "{}", text(&granted.stderr));
    (agent, other)
}

/// #59, Theo's choice A for #858: one session at a time uses a login account, so a second
/// session passes it over for the next listed account, naming the session using it.
#[tokio::test(flavor = "multi_thread")]
async fn a_login_another_session_uses_is_passed_over_naming_that_session() {
    let home = Home::new();
    let (agent, other) = agent_on_work_login_with_other_project(&home).await;
    let client = home.connect().await;

    let second = new_session(&client, &other).await;

    let launch = listed(&client, &second.id)
        .await
        .launch
        .expect("the session launched");
    assert_eq!(launch.account.as_deref(), Some("spare"));
    assert_eq!(
        launch.passed_over,
        [format!(
            "work (session {} uses its login: one session at a time uses a login account)",
            agent.session
        )]
    );
}

/// #59: a session on an account's credential shares no sign-in, so once the account is a
/// login instead, a second session takes that login.
#[tokio::test(flavor = "multi_thread")]
async fn a_session_on_an_accounts_credential_does_not_hold_its_login() {
    let home = Home::new();
    let agent = agent_with_logins(&home, WORK_LOGIN, &[], |home| {
        home.write_account_credential(
            "claude",
            "work",
            "ANTHROPIC_API_KEY",
            &format!("{SETUP_TOKEN}\n"),
            0o600,
        );
    })
    .await;
    assert_eq!(
        launch_account(&home, &agent.session).await.as_deref(),
        Some("work")
    );
    std::fs::remove_file(
        home.path()
            .join("config")
            .join(BUILD_PROFILE_DIR)
            .join(cockpit_config::PROVIDER_CREDENTIALS_DIR)
            .join("claude/work/ANTHROPIC_API_KEY"),
    )
    .unwrap();
    write_claude_login(&home, "work", "fixture-login-v1");
    let client = home.connect().await;
    let (other, _) = home.project(&client, "other").await;
    let granted = home
        .cockpit(&["account", "grant", "other", "claude", "work"], &[])
        .await;
    assert!(granted.status.success(), "{}", text(&granted.stderr));

    let second = new_session(&client, &other).await;

    let launch = listed(&client, &second.id).await.launch.unwrap();
    assert_eq!(launch.account.as_deref(), Some("work"));
    assert!(launch.passed_over.is_empty(), "{:?}", launch.passed_over);
}

/// #59, Theo's choice A for #858: two sessions launching at once in projects granted the same
/// login never both take it; the launch recorded second passes it over or is refused.
#[tokio::test(flavor = "multi_thread")]
async fn two_sessions_launching_at_once_never_both_take_one_login() {
    let home = Home::new();
    home.write_owner_config(&format!("{REQUIRED}{TWO_ACCOUNTS}"));
    write_claude_login(&home, "work", "fixture-login-v1");
    home.write_account_credential(
        "claude",
        "spare",
        "ANTHROPIC_API_KEY",
        &format!("{SETUP_TOKEN}\n"),
        0o600,
    );
    let _daemon = home.start();
    let client = home.connect().await;
    let mut roots = Vec::new();
    for project in ["first", "second"] {
        let (root, _) = home.project(&client, project).await;
        let granted = home
            .cockpit(&["account", "grant", project, "claude", "work"], &[])
            .await;
        assert!(granted.status.success(), "{}", text(&granted.stderr));
        roots.push(root);
    }
    let (first, second) = (home.connect().await, home.connect().await);

    let (started_first, started_second) = tokio::join!(
        first.request(session_new_command(&roots[0]), Duration::from_secs(60)),
        second.request(session_new_command(&roots[1]), Duration::from_secs(60)),
    );

    let mut on_work = 0;
    for started in [started_first, started_second] {
        match started {
            Ok(cockpit_protocol::Reply::Session { session }) => {
                if launch_account(&home, &session.id).await.as_deref() == Some("work") {
                    on_work += 1;
                }
            }
            Err(ClientError::Daemon(error)) => assert!(
                error
                    .message
                    .contains("one session at a time uses a login account"),
                "{error:?}"
            ),
            other => panic!("session-new answered {other:?}"),
        }
    }
    assert_eq!(on_work, 1);
}

/// #59, Theo's choice A for #858: a session asleep holds no login, and waking it on a login
/// another session took meanwhile is refused, naming that session.
#[tokio::test(flavor = "multi_thread")]
async fn waking_a_session_on_a_login_another_session_took_is_refused_naming_it() {
    let home = Home::new();
    let (agent, other) = agent_on_work_login_with_other_project(&home).await;
    let client = home.connect().await;
    client
        .request(
            Command::SessionSleep {
                session: agent.session.clone(),
            },
            GENEROUS,
        )
        .await
        .unwrap();
    let second = new_session(&client, &other).await;
    let taken = listed(&client, &second.id).await.launch.unwrap();
    assert_eq!(taken.account.as_deref(), Some("work"));

    let woken = client
        .request(
            Command::SessionWake {
                session: agent.session.clone(),
            },
            GENEROUS,
        )
        .await;

    assert!(
        matches!(&woken, Err(ClientError::Daemon(error))
        if error.code == ErrorCode::Refused
            && error.message.contains(&format!(
                "account work, which has its login in use by session {} (one session at a \
                 time uses a login account)",
                second.id
            ))),
        "{woken:?}"
    );
}

/// #59, owner decision C: `session new` says which login it passed over and the grant.
#[tokio::test(flavor = "multi_thread")]
async fn session_new_names_the_login_it_passed_over_and_the_grant() {
    let home = Home::new();
    home.write_owner_config(&format!("{REQUIRED}{TWO_ACCOUNTS}"));
    write_claude_login(&home, "work", "fixture-login-v1");
    home.write_account_credential("claude", "spare", "ANTHROPIC_API_KEY", SETUP_TOKEN, 0o600);
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, COMMANDED_PROJECT).await;

    let started = home
        .cockpit(
            &[
                "session",
                "new",
                root.to_str().unwrap(),
                "--agent",
                "claude",
            ],
            &[],
        )
        .await;

    let told = text(&started.stdout);
    assert!(started.status.success(), "{}", text(&started.stderr));
    assert!(
        told.contains(&format!("passed over: {PASSED_OVER_WORK}\n")),
        "{told}"
    );
}

/// #59, owner decision C: once the owner revokes the grant, a restart kept on that login is
/// refused naming the grant, never moved to another account.
#[tokio::test(flavor = "multi_thread")]
async fn a_restart_on_a_login_whose_grant_was_revoked_is_refused_naming_the_grant() {
    let home = Home::new();
    let agent = agent_with_logins(&home, TWO_ACCOUNTS, &["work"], |home| {
        write_claude_login(home, "work", "fixture-login-v1");
        home.write_account_credential(
            "claude",
            "spare",
            "ANTHROPIC_API_KEY",
            &format!("{SETUP_TOKEN}\n"),
            0o600,
        );
    })
    .await;
    run_in_agent(&home, &agent.root, "true").await;
    let revoked = home.cockpit(&["account", "revoke", "1"], &[]).await;
    assert!(revoked.status.success(), "{}", text(&revoked.stderr));

    let restarted = restart(&home, &agent.session).await;

    assert!(
        matches!(&restarted, Err(ClientError::Daemon(error))
        if error.code == ErrorCode::Refused
            && error.message.contains(
                "is a login account project commanded holds no grant for \
                 (`torlek account grant commanded claude work` lets it)"
            )),
        "{restarted:?}"
    );
}

/// #59: an account holding both a credential file and a signed-in login refuses the launch,
/// naming it: which one to use is the owner's to say.
#[tokio::test(flavor = "multi_thread")]
async fn an_account_with_both_a_credential_and_a_login_refuses_the_launch_naming_it() {
    let home = Home::new();
    home.write_owner_config(&format!("{REQUIRED}{WORK_LOGIN}"));
    write_claude_login(&home, "work", "fixture-login-v1");
    home.write_account_credential("claude", "work", "ANTHROPIC_API_KEY", SETUP_TOKEN, 0o600);
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, COMMANDED_PROJECT).await;
    home.cockpit(&grant("work"), &[]).await;

    let started = client
        .request(session_new_command(&root), Duration::from_secs(60))
        .await;

    assert!(
        matches!(&started, Err(ClientError::Daemon(error))
        if error.code == ErrorCode::Refused
            && error.message.contains(
                "the claude provider's account work holds both a credential file and a \
                 signed-in login"
            )),
        "{started:?}"
    );
}

/// #59: a login account whose login is no plain file refuses a new launch, naming the file
/// and the sign-in that replaces it.
#[tokio::test(flavor = "multi_thread")]
async fn a_login_that_is_no_plain_file_refuses_the_launch_naming_it() {
    let home = Home::new();
    home.write_owner_config(&format!("{REQUIRED}{WORK_LOGIN}"));
    let file = write_claude_login(&home, "work", "fixture-login-v1");
    std::fs::remove_file(&file).unwrap();
    std::fs::create_dir(&file).unwrap();
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, COMMANDED_PROJECT).await;
    home.cockpit(&grant("work"), &[]).await;

    let started = client
        .request(session_new_command(&root), Duration::from_secs(60))
        .await;

    assert!(
        matches!(&started, Err(ClientError::Daemon(error))
        if error.code == ErrorCode::Refused
            && error.message.contains(&format!(
                "the claude provider's account work keeps a login {} that is no plain file, so \
                 its sandboxed sessions do not launch: sign it in again",
                file.display()
            ))),
        "{started:?}"
    );
}

/// #59: an unsandboxed agent signs in from the owner's home, so its launch runs on no account.
#[tokio::test(flavor = "multi_thread")]
async fn an_unsandboxed_launch_runs_on_no_account() {
    let home = Home::new();
    home.write_owner_config(TWO_ACCOUNTS);
    home.write_account_credential("claude", "work", "ANTHROPIC_API_KEY", SETUP_TOKEN, 0o600);
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, "app").await;
    let session = new_session(&client, &root).await;
    home.wait_for_launches(&session.id, 1).await;

    let account = launch_account(&home, &session.id).await;

    assert_eq!(account, None);
}

/// #968: with no credential, `session new` and `doctor` name the files one may go in.
#[tokio::test(flavor = "multi_thread")]
async fn without_a_credential_session_new_and_doctor_name_the_files_to_write() {
    let home = Home::new();
    home.write_owner_config(REQUIRED);
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, COMMANDED_PROJECT).await;
    let files = home
        .path()
        .join("config")
        .join(BUILD_PROFILE_DIR)
        .join(cockpit_config::PROVIDER_CREDENTIALS_DIR)
        .join("claude");

    let started = home
        .cockpit(
            &[
                "session",
                "new",
                root.to_str().unwrap(),
                "--agent",
                "claude",
            ],
            &[],
        )
        .await;
    let doctor = home.cockpit(&["doctor"], &[]).await;

    let told = text(&started.stderr);
    assert!(started.status.success(), "{told}");
    let sign_in = told
        .lines()
        .find_map(|line| line.strip_prefix("sign-in: "))
        .unwrap_or_else(|| panic!("no sign-in line in:\n{told}"));
    assert!(
        sign_in.contains(&files.join("CLAUDE_CODE_OAUTH_TOKEN").display().to_string()),
        "{sign_in}"
    );
    let report = text(&doctor.stdout);
    let row = report
        .lines()
        .find(|line| line.contains("provider claude sandboxed sign-in"))
        .unwrap_or_else(|| panic!("no sign-in row in:\n{report}"));
    assert!(row.trim_start().starts_with("needs-authorization"), "{row}");
    assert!(
        report.contains(&files.join("ANTHROPIC_API_KEY").display().to_string()),
        "{report}"
    );
}

/// `doctor`'s report and its `sandboxed sign-in` row for the claude account `account`.
async fn doctor_sign_in_row(home: &Home, account: &str) -> (String, String) {
    let doctor = home.cockpit(&["doctor"], &[]).await;
    let report = text(&doctor.stdout);
    let row = report
        .lines()
        .find(|line| {
            line.contains(&format!(
                "provider claude account {account} sandboxed sign-in"
            ))
        })
        .unwrap_or_else(|| panic!("no sign-in row in:\n{report}"))
        .trim_start()
        .to_owned();
    (report, row)
}

/// #59: `doctor` shows a granted login account as a sandboxed sign-in, naming its file and
/// the projects granted it, never what it holds.
#[tokio::test(flavor = "multi_thread")]
async fn doctor_shows_a_granted_login_account_as_its_sandboxed_sign_in() {
    let home = Home::new();
    home.write_owner_config(&format!("{REQUIRED}{WORK_LOGIN}"));
    let file = write_claude_login(&home, "work", "fixture-login-v1");
    let _daemon = home.start();
    let client = home.connect().await;
    home.project(&client, COMMANDED_PROJECT).await;
    let granted = home.cockpit(&grant("work"), &[]).await;
    assert!(granted.status.success(), "{}", text(&granted.stderr));

    let (report, row) = doctor_sign_in_row(&home, "work").await;

    assert!(row.starts_with("available"), "{row}");
    assert!(
        report.contains(&format!(
            "signed-in login {}: sessions of project {COMMANDED_PROJECT} use it",
            file.display()
        )),
        "{report}"
    );
    assert!(!report.contains("fixture-login-v1"), "{report}");
}

/// #59: no session uses a login account no project holds a grant for, so `doctor` names the
/// grant to give.
#[tokio::test(flavor = "multi_thread")]
async fn doctor_names_the_grant_a_login_account_no_project_holds_needs() {
    let home = Home::new();
    home.write_owner_config(&format!("{REQUIRED}{WORK_LOGIN}"));
    write_claude_login(&home, "work", "fixture-login-v1");
    let _daemon = home.start();

    let (report, row) = doctor_sign_in_row(&home, "work").await;

    assert!(row.starts_with("needs-authorization"), "{row}");
    assert!(
        report.contains("account grant PROJECT claude work`"),
        "{report}"
    );
}

/// #59: a login that is no plain file refuses its launches, so `doctor` blocks on its row
/// and still reports the other accounts.
#[tokio::test(flavor = "multi_thread")]
async fn doctor_blocks_on_a_login_that_is_no_plain_file_and_keeps_the_other_rows() {
    let home = Home::new();
    home.write_owner_config(&format!("{REQUIRED}{TWO_ACCOUNTS}"));
    let file = write_claude_login(&home, "work", "fixture-login-v1");
    std::fs::remove_file(&file).unwrap();
    std::fs::create_dir(&file).unwrap();
    let _daemon = home.start();

    let (report, row) = doctor_sign_in_row(&home, "work").await;

    assert!(row.starts_with("missing-prerequisite"), "{row}");
    assert!(
        report.contains(&format!(
            "keeps a login {} that is no plain file, so its sandboxed sessions do not launch",
            file.display()
        )),
        "{report}"
    );
    assert!(
        report.contains("provider claude account spare sandboxed sign-in"),
        "{report}"
    );
}

/// #59: with no account holding a credential, `session new` names the first listed account
/// and its own folder's files.
#[tokio::test(flavor = "multi_thread")]
async fn without_a_credential_session_new_names_the_first_account_and_its_files() {
    let home = Home::new();
    home.write_owner_config(&format!("{REQUIRED}{TWO_ACCOUNTS}"));
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, COMMANDED_PROJECT).await;
    let work = home
        .path()
        .join("config")
        .join(BUILD_PROFILE_DIR)
        .join(cockpit_config::PROVIDER_CREDENTIALS_DIR)
        .join("claude/work");

    let started = home
        .cockpit(
            &[
                "session",
                "new",
                root.to_str().unwrap(),
                "--agent",
                "claude",
            ],
            &[],
        )
        .await;

    let told = text(&started.stderr);
    assert!(started.status.success(), "{told}");
    let sign_in = told
        .lines()
        .find_map(|line| line.strip_prefix("sign-in: "))
        .unwrap_or_else(|| panic!("no sign-in line in:\n{told}"));
    assert!(
        sign_in.contains("credential for account work,"),
        "{sign_in}"
    );
    assert!(
        sign_in.contains(&work.join("CLAUDE_CODE_OAUTH_TOKEN").display().to_string()),
        "{sign_in}"
    );
}

/// #59: a credential file beside a signed-in login would refuse the launch, so `session new`
/// asks for one for the next account and leaves the login to its grant.
#[tokio::test(flavor = "multi_thread")]
async fn session_new_never_asks_a_signed_in_login_account_for_a_credential() {
    let home = Home::new();
    home.write_owner_config(&format!("{REQUIRED}{TWO_ACCOUNTS}"));
    write_claude_login(&home, "work", "fixture-login-v1");
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, COMMANDED_PROJECT).await;

    let started = home
        .cockpit(
            &[
                "session",
                "new",
                root.to_str().unwrap(),
                "--agent",
                "claude",
            ],
            &[],
        )
        .await;

    let told = text(&started.stderr);
    assert!(started.status.success(), "{told}");
    let sign_in = told
        .lines()
        .find_map(|line| line.strip_prefix("sign-in: "))
        .unwrap_or_else(|| panic!("no sign-in line in:\n{told}"));
    assert!(
        sign_in.contains("credential for account spare,"),
        "{sign_in}"
    );
}

/// The folder trust Claude reads from `config`, the `.claude.json` of a session's home, for
/// the project at `root`; `None` when the file or its entry is missing.
fn claude_trust(config: &Path, root: &Path) -> Option<bool> {
    let text = std::fs::read_to_string(config).ok()?;
    let config: serde_json::Value = serde_json::from_str(&text).unwrap();
    config["projects"][root.to_str().unwrap()]["hasTrustDialogAccepted"].as_bool()
}

/// #968: the folder trust survives to the next sandboxed start of the same provider in the
/// same project: Torlek writes it from `projects.trusted` into each session's own Claude
/// config, never the owner's. A session's home goes when it ends, so each is read while its
/// session runs.
#[tokio::test(flavor = "multi_thread")]
async fn a_trusted_projects_folder_trust_reaches_each_sandboxed_session_of_it() {
    let home = Home::new();
    let root = home.path().join("projects").join(COMMANDED_PROJECT);
    let agent = sandboxed_agent(&home, |home| {
        home.write_owner_config(&format!(
            "{REQUIRED}[projects]\ntrusted = [{:?}]\n",
            root.to_str().unwrap()
        ));
    })
    .await;
    let homes = home.state_dir().join("homes");
    run_in_agent(&home, &agent.root, "true").await;
    let first = claude_trust(&homes.join(&agent.session).join(".claude.json"), &root);
    let client = home.connect().await;
    let ended = client
        .request(
            Command::SessionEnd {
                session: agent.session.clone(),
            },
            GENEROUS,
        )
        .await;
    assert!(ended.is_ok(), "{ended:?}");
    std::fs::remove_dir_all(agent.root.join(".agent")).unwrap();

    let next = new_session(&client, &agent.root).await;
    run_in_agent(&home, &agent.root, "true").await;

    assert_eq!(first, Some(true));
    let config = homes.join(&next.id).join(".claude.json");
    assert_eq!(claude_trust(&config, &root), Some(true), "{config:?}");
    assert!(!home.path().join(".claude.json").exists());
}

/// #968: a project the owner did not trust gets no folder trust written for it.
#[tokio::test(flavor = "multi_thread")]
async fn a_project_not_in_projects_trusted_gets_no_folder_trust() {
    let home = Home::new();
    let agent = sandboxed_agent(&home, |_| {}).await;

    run_in_agent(&home, &agent.root, "true").await;

    let config = home
        .state_dir()
        .join("homes")
        .join(&agent.session)
        .join(".claude.json");
    assert_eq!(claude_trust(&config, &agent.root), None, "{config:?}");
}

/// #1178: a sandboxed session's home starts past Claude Code's first-run screens, in a project
/// the owner did not trust too: its onboarding is done, and the key the agent holds, the
/// placeholder, is approved, so the agent reaches its first turn with nobody at its terminal.
#[tokio::test(flavor = "multi_thread")]
async fn a_sandboxed_sessions_home_starts_past_claude_codes_first_run_screens() {
    let home = Home::new();
    let agent = sandboxed_agent(&home, |home| {
        home.write_provider_credential(
            "claude",
            "ANTHROPIC_API_KEY",
            "sk-ant-api03-fixture_value\n",
            0o600,
        );
    })
    .await;

    run_in_agent(&home, &agent.root, "true").await;

    let config = home
        .state_dir()
        .join("homes")
        .join(&agent.session)
        .join(".claude.json");
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
    assert_eq!(written["hasCompletedOnboarding"], true, "{config:?}");
    assert_eq!(
        written["customApiKeyResponses"]["approved"],
        serde_json::json!([cockpit_claude::first_run::key_tail(
            cockpit_sandbox::PLACEHOLDER
        )]),
        "{config:?}"
    );
}

/// What the owner's `cockpit network VERB...` printed; it must succeed.
async fn owner_network(home: &Home, arguments: &[&str]) -> String {
    let mut words = vec!["network"];
    words.extend_from_slice(arguments);
    let output = home.cockpit(&words, &[]).await;
    assert!(
        output.status.success(),
        "{arguments:?}: {}",
        text(&output.stderr)
    );
    text(&output.stdout)
}

/// The proxy's answer to [`ASKED_HOST`] once `is_done` accepts it: a decision reaches the
/// launcher within its poll.
async fn connect_until(home: &Home, root: &Path, is_done: impl Fn(&str) -> bool) -> String {
    let deadline = std::time::Instant::now() + GENEROUS;
    loop {
        let printed = run_in_agent(home, root, &connect_line(ASKED_HOST)).await;
        if is_done(&printed) || std::time::Instant::now() > deadline {
            return printed;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn is_refused(printed: &str) -> bool {
    printed.starts_with("HTTP/1.1 403 Forbidden")
}

/// #674: the refusal comes at once and says the owner is asked and how it is unblocked; the
/// request is in the owner's network list and approval list.
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_host_is_asked_of_the_owner_and_listed_for_them() {
    let home = Home::new();
    let agent = sandboxed_agent(&home, |_| {}).await;

    let printed = run_in_agent(&home, &agent.root, &connect_line(ASKED_HOST)).await;

    assert!(is_refused(&printed), "{printed}");
    assert!(
        printed.contains(
            "unknown.invalid:443 is not allowed: no entry of the allowlist names it; the owner \
             is asked: `torlek network list` shows the request, `torlek network allow` lets it \
             through"
        ),
        "{printed}"
    );
    assert!(milliseconds(&printed) < 1000, "{printed}");
    let listed = network_list_once_it_contains(&home, &asking_line(&agent.session)).await;
    assert!(listed.contains(&asking_line(&agent.session)), "{listed}");
    let approvals = home.cockpit(&["approval", "list"], &[]).await;
    assert!(
        text(&approvals.stdout).contains(&asking_line(&agent.session)),
        "{}{}",
        text(&approvals.stdout),
        text(&approvals.stderr)
    );
}

/// #966: once the daemon filed the request, a retry reads its number and the owner's command
/// on the status line, the part of a failed `CONNECT` that clients print.
#[tokio::test(flavor = "multi_thread")]
async fn a_retry_names_the_filed_request_and_the_owners_command_on_its_status_line() {
    let home = Home::new();
    let agent = sandboxed_agent(&home, |_| {}).await;
    run_in_agent(&home, &agent.root, &connect_line(ASKED_HOST)).await;

    let printed = connect_until(&home, &agent.root, |printed| {
        printed.contains("network request 1")
    })
    .await;

    let status_line = printed.lines().next().unwrap_or_default();
    assert_eq!(
        status_line,
        format!(
            "HTTP/1.1 403 Forbidden: torlek-sandbox: {ASKED_HOST} is not allowed: no entry of \
             the allowlist names it; the owner is asked in network request 1: \
             `torlek network allow 1` lets it through"
        ),
        "{printed}"
    );
}

/// #674: "allow once" lets the session's retry through: the proxy goes to the host instead
/// of refusing it, for the rest of this session.
#[tokio::test(flavor = "multi_thread")]
async fn allow_once_lets_the_retry_through() {
    let home = Home::new();
    let agent = sandboxed_agent(&home, |_| {}).await;
    asked_and_listed(&home, &agent).await;

    let allowed = owner_network(&home, &["allow", "1"]).await;
    let printed = connect_until(&home, &agent.root, |printed| !is_refused(printed)).await;

    assert!(
        allowed.contains("allowed for the rest of this session by the owner through the CLI"),
        "{allowed}"
    );
    assert!(!is_refused(&printed), "{printed}");
    assert!(printed.contains("error=dns_error"), "{printed}");
    assert_eq!(owner_network(&home, &["list"]).await, "");
}

/// #674: "allow always" is a project grant kept in the state database: after a daemon
/// restart, a new session of the project is let through without asking.
#[tokio::test(flavor = "multi_thread")]
async fn allow_always_survives_a_restart_into_a_new_session_of_the_project() {
    let home = Home::new();
    let agent = sandboxed_agent(&home, |_| {}).await;
    asked_and_listed(&home, &agent).await;
    owner_network(&home, &["allow", "1", "--always"]).await;
    let Sandboxed {
        _daemon: daemon,
        root,
        session,
    } = agent;

    daemon.kill();
    let _daemon = home.start();
    let client = home.connect().await;
    let ended = client
        .request(Command::SessionEnd { session }, GENEROUS)
        .await;
    assert!(ended.is_ok(), "{ended:?}");
    std::fs::remove_dir_all(root.join(".agent")).unwrap();
    new_session(&client, &root).await;
    let printed = run_in_agent(&home, &root, &connect_line(ASKED_HOST)).await;

    assert!(!is_refused(&printed), "{printed}");
    let grants = owner_network(&home, &["grants"]).await;
    assert!(
        grants.contains(&format!(
            "grant\t1\t{COMMANDED_PROJECT}\t{ASKED_HOST}\trequest 1"
        )),
        "{grants}"
    );
}

/// #674: a denied host keeps being refused at once, naming the owner's denial, and is not
/// asked about again.
#[tokio::test(flavor = "multi_thread")]
async fn a_denied_host_keeps_being_refused_at_once() {
    let home = Home::new();
    let agent = sandboxed_agent(&home, |_| {}).await;
    asked_and_listed(&home, &agent).await;

    owner_network(&home, &["deny", "1"]).await;
    let printed = connect_until(&home, &agent.root, |printed| {
        printed.contains("the owner denied network request 1")
    })
    .await;

    assert!(is_refused(&printed), "{printed}");
    assert!(
        printed.contains("unknown.invalid:443 is not allowed: the owner denied network request 1"),
        "{printed}"
    );
    assert!(milliseconds(&printed) < 1000, "{printed}");
    assert_eq!(owner_network(&home, &["list"]).await, "");
}

/// #1252: the open list drops a decided request; `list --all` keeps it with the owner's
/// decision and the seconds the session waited.
#[tokio::test(flavor = "multi_thread")]
async fn the_decided_list_keeps_a_denied_request_the_open_list_dropped() {
    let home = Home::new();
    let agent = sandboxed_agent(&home, |_| {}).await;
    asked_and_listed(&home, &agent).await;
    owner_network(&home, &["deny", "1"]).await;

    let decided = owner_network(&home, &["list", "--all"]).await;

    let prefix = format!(
        "network\t1\tdenied\t{}\t{COMMANDED_PROJECT}\t{ASKED_HOST}\t",
        agent.session
    );
    let waited = decided
        .strip_prefix(&prefix)
        .unwrap_or_else(|| panic!("{decided}"));
    assert!(
        waited
            .trim_end()
            .strip_suffix('s')
            .is_some_and(|seconds| seconds.parse::<u32>().is_ok()),
        "{decided}"
    );
    assert_eq!(owner_network(&home, &["list"]).await, "");
}

/// #674 regression: no path adds an entry without the owner. Every network verb is refused
/// on the agents' socket; the request still asks, no grant is made, the host stays refused.
#[tokio::test(flavor = "multi_thread")]
async fn no_network_verb_on_the_agent_socket_allows_a_host() {
    let home = Home::new();
    let agent = sandboxed_agent(&home, |_| {}).await;
    asked_and_listed(&home, &agent).await;
    let socket = Connection::open(&home.agent_socket_path(), GENEROUS)
        .await
        .unwrap();

    for command in [
        Command::NetworkList,
        Command::NetworkDecided,
        Command::NetworkAllow {
            request: 1,
            always: true,
            through: None,
        },
        Command::NetworkDeny {
            request: 1,
            through: None,
        },
        Command::NetworkGrants { project: None },
        Command::NetworkGrantGithub {
            project: COMMANDED_PROJECT.to_owned(),
            through: None,
        },
        Command::NetworkRevoke {
            grant: 1,
            through: None,
        },
    ] {
        let refused = socket.request(command, GENEROUS).await;

        assert!(
            matches!(&refused, Err(ClientError::Daemon(error)) if error.code == ErrorCode::NotAuthorized),
            "{refused:?}"
        );
    }
    assert!(
        owner_network(&home, &["list"])
            .await
            .contains(&asking_line(&agent.session))
    );
    assert_eq!(owner_network(&home, &["grants"]).await, "");
    let still = run_in_agent(&home, &agent.root, &connect_line(ASKED_HOST)).await;
    assert!(is_refused(&still), "{still}");
}

/// What `doctor` printed for the `sandbox` requirement and the `next:` line under it.
fn doctor_sandbox_row(report: &str) -> (&str, &str) {
    let mut lines = report.lines();
    let row = lines
        .by_ref()
        .find(|line| line.contains("  sandbox: "))
        .unwrap_or_else(|| panic!("no sandbox requirement in:\n{report}"));
    let next = lines
        .next()
        .and_then(|line| line.trim_start().strip_prefix("next: "))
        .unwrap_or_else(|| panic!("no next action under {row}"));
    (row, next)
}

/// #965: with only `sandbox = "required"` set, a shared desktop session makes the sandbox
/// refuse every launch; `doctor` and `init` say so with the refusal's own fix instead of
/// reporting the setup as ready.
#[tokio::test(flavor = "multi_thread")]
async fn a_required_sandbox_with_a_shared_desktop_blocks_doctor_and_init_as_the_launch_refuses() {
    let home = Home::new();
    home.write_owner_config(REQUIRED);
    let _daemon = home.start_with(&[("DISPLAY", ":1")]);
    let client = home.connect().await;
    let (root, _) = home.project(&client, "app").await;

    let refused = client
        .request(session_new_command(&root), Duration::from_secs(60))
        .await;
    let doctor = home.cockpit(&["doctor"], &[]).await;
    let init = home.cockpit(&["init"], &[]).await;

    let Err(ClientError::Daemon(refusal)) = &refused else {
        panic!("{refused:?}");
    };
    assert!(
        refusal.message.contains("desktop session shared"),
        "{refusal:?}"
    );
    let report = text(&doctor.stdout);
    assert_eq!(doctor.status.code(), Some(1), "{report}");
    let (row, next) = doctor_sandbox_row(&report);
    assert!(
        row.trim_start().starts_with("missing-prerequisite"),
        "{row}"
    );
    assert!(row.contains("DISPLAY names /tmp/.X11-unix/X1"), "{row}");
    assert!(
        next.contains("agents.desktop_session = \"withheld\""),
        "{next}"
    );
    let checklist = text(&init.stdout);
    assert!(
        checklist.contains("DISPLAY names /tmp/.X11-unix/X1"),
        "{checklist}"
    );
    assert!(
        checklist.contains("agents.desktop_session = \"withheld\""),
        "{checklist}"
    );
}

/// #965: the fix the report names makes the sandbox ready, with no restart.
#[tokio::test(flavor = "multi_thread")]
async fn withholding_the_desktop_session_makes_a_required_sandbox_ready_after_a_reload() {
    let home = Home::new();
    home.write_owner_config(REQUIRED);
    let _daemon = home.start_with(&[("DISPLAY", ":1")]);
    home.write_owner_config("[agents]\nsandbox = \"required\"\ndesktop_session = \"withheld\"\n");

    let reload = home.cockpit(&["config", "reload"], &[]).await;
    let doctor = home.cockpit(&["doctor"], &[]).await;

    assert!(reload.status.success(), "{}", text(&reload.stderr));
    let report = text(&doctor.stdout);
    let sandbox = report
        .lines()
        .find(|line| line.contains("  sandbox: "))
        .unwrap_or_else(|| panic!("no sandbox requirement in:\n{report}"));
    assert!(sandbox.trim_start().starts_with("available"), "{report}");
}

/// What a reboot leaves: the daemon killed, then every agent with its tmux server, so the
/// next daemon restores their sessions.
fn reboot(home: &Home, daemon: Daemon) {
    daemon.kill();
    let killed = std::process::Command::new("tmux")
        .env_remove("TMUX")
        .arg("-S")
        .arg(home.tmux_socket_of(BUILD_PROFILE_DIR))
        .arg("kill-server")
        .output()
        .unwrap();
    assert!(killed.status.success(), "{}", text(&killed.stderr));
}

/// #969: a session first launched with the sandbox off, rebooted, and restored by a daemon
/// whose owner since set `required`: the daemon running and the session as it restored it.
async fn restored_after_turning_the_sandbox_on(home: &Home) -> (Daemon, SessionView) {
    let daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, "app").await;
    let started = new_session(&client, &root).await;
    let agent = home.wait_for_launches(&started.id, 1).await[0].clone();
    drop(client);
    reboot(home, daemon);
    let deadline = std::time::Instant::now() + GENEROUS;
    while is_alive(agent.pid) {
        assert!(
            std::time::Instant::now() < deadline,
            "the agent outlived its tmux server"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    home.write_owner_config(REQUIRED);

    let daemon = home.start();
    let client = home.connect().await;
    // Restored before the daemon accepts a request.
    let restored = listed(&client, &started.id).await;
    (daemon, restored)
}

/// #969: its conversation is in the owner's home, which a sandboxed agent never sees, so the
/// restore is refused before launch, naming the change, instead of an agent that fails.
#[tokio::test(flavor = "multi_thread")]
async fn a_session_first_launched_unsandboxed_is_refused_a_sandboxed_restore() {
    let home = Home::new();

    let (_daemon, restored) = restored_after_turning_the_sandbox_on(&home).await;

    assert_eq!(restored.state, SessionStateView::Failed, "{restored:?}");
    let reason = restored.reason.unwrap_or_default();
    assert!(
        reason.starts_with("restore refused: ")
            && reason.contains("first launched with the sandbox off")
            && reason.contains("agents.sandbox = \"off\""),
        "{reason}"
    );
    assert_eq!(
        home.launches_of(&restored.id).len(),
        1,
        "never launched again"
    );
}

/// #969: the owner's own resume is refused the same way while the setting differs.
#[tokio::test(flavor = "multi_thread")]
async fn a_session_first_launched_unsandboxed_is_refused_a_sandboxed_resume() {
    let home = Home::new();
    let (_daemon, restored) = restored_after_turning_the_sandbox_on(&home).await;
    let client = home.connect().await;

    let resumed = client
        .request(
            Command::SessionResume {
                session: restored.id.clone(),
            },
            GENEROUS,
        )
        .await;

    assert!(
        matches!(&resumed, Err(ClientError::Daemon(error))
            if error.code == ErrorCode::Conflict
                && error.message.contains("first launched with the sandbox off")),
        "{resumed:?}"
    );
    assert_eq!(
        listed(&client, &restored.id).await.state,
        SessionStateView::Failed
    );
    assert_eq!(
        home.launches_of(&restored.id).len(),
        1,
        "never launched again"
    );
}

/// #969: the refusal keeps the conversation: with the sandbox off again, a resume reopens it.
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_session_resumes_its_conversation_once_the_sandbox_is_off_again() {
    let home = Home::new();
    let (daemon, restored) = restored_after_turning_the_sandbox_on(&home).await;
    daemon.kill();
    home.write_owner_config("");
    let _daemon = home.start();
    let client = home.connect().await;

    let resumed = client
        .request(
            Command::SessionResume {
                session: restored.id.clone(),
            },
            GENEROUS,
        )
        .await;

    assert!(resumed.is_ok(), "{resumed:?}");
    home.wait_for_launches(&restored.id, 2).await;
    let conversation = restored
        .conversation
        .expect("Claude Code names it at launch");
    let arguments = home.arguments_of(&restored.id);
    assert!(
        arguments
            .last()
            .is_some_and(|last| last.contains(&format!("--resume {conversation}"))),
        "{arguments:?}"
    );
}

/// #969, the reverse: a sandboxed session's conversation is in the session's own home, which
/// an unsandboxed agent does not use, so restoring it with the sandbox off is refused too.
#[tokio::test(flavor = "multi_thread")]
async fn a_session_first_launched_sandboxed_is_refused_an_unsandboxed_restore() {
    let home = Home::new();
    let Sandboxed {
        _daemon: daemon,
        root,
        session,
    } = sandboxed_agent(&home, |_| {}).await;
    // A sandboxed agent cannot write the launch log: its line proves it ran.
    assert_eq!(run_in_agent(&home, &root, "true").await, "exit=0\n");
    let launched = listed(&home.connect().await, &session).await.launch;
    reboot(&home, daemon);
    home.write_owner_config("");

    let _daemon = home.start();
    let client = home.connect().await;
    let restored = listed(&client, &session).await;

    assert_eq!(restored.state, SessionStateView::Failed, "{restored:?}");
    let reason = restored.reason.unwrap_or_default();
    assert!(
        reason.starts_with("restore refused: ")
            && reason.contains("first launched sandboxed")
            && reason.contains("agents.sandbox = \"required\""),
        "{reason}"
    );
    assert_eq!(restored.launch, launched, "never launched again");
}

/// #969: a restart after the owner turned the sandbox on is refused before its stop, so the
/// agent keeps running its conversation.
#[tokio::test(flavor = "multi_thread")]
async fn a_restart_after_turning_the_sandbox_on_is_refused_and_keeps_the_agent() {
    let home = Home::new();
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, "app").await;
    let started = new_session(&client, &root).await;
    let agent = home.wait_for_launches(&started.id, 1).await[0].clone();
    home.write_owner_config(REQUIRED);
    let reloaded = home.cockpit(&["config", "reload"], &[]).await;
    assert!(reloaded.status.success(), "{}", text(&reloaded.stderr));

    let restarted = client
        .request(
            Command::SessionRestart {
                session: started.id.clone(),
            },
            GENEROUS,
        )
        .await;

    assert!(
        matches!(&restarted, Err(ClientError::Daemon(error))
            if error.code == ErrorCode::Conflict
                && error.message.contains("first launched with the sandbox off")),
        "{restarted:?}"
    );
    assert_eq!(
        listed(&client, &started.id).await.state,
        SessionStateView::Running
    );
    assert!(is_alive(agent.pid), "the agent kept running");
}

/// #880: where the agent connects to the declared service, inside the sandbox.
const SERVICE_LISTEN: u16 = 4100;

/// A host service on a loopback port that answers each connection with `pong` and keeps it
/// open: what a declared service's forward relays to once the owner allowed it. Only the
/// relay closes a connection the agent holds.
async fn host_service() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut connection, _)) = listener.accept().await {
            tokio::spawn(async move {
                use tokio::io::AsyncWriteExt;
                let _ = connection.write_all(b"pong\n").await;
                tokio::time::sleep(Duration::from_secs(120)).await;
            });
        }
    });
    port
}

/// The project file declaring the `echo` service, relayed to the host's `target` port.
fn services_file(target: u16) -> String {
    format!(
        "[[services]]\nname = \"echo\"\nlisten = \"127.0.0.1:{SERVICE_LISTEN}\"\n\
         target = \"127.0.0.1:{target}\"\n"
    )
}

fn echo_record(target: u16) -> String {
    format!("service echo listen 127.0.0.1:{SERVICE_LISTEN} target 127.0.0.1:{target}")
}

/// A sandboxed agent whose project declares the `echo` service relayed to `target`.
async fn agent_declaring_echo(home: &Home, target: u16) -> Sandboxed {
    sandboxed_agent(home, |home| {
        let config = commanded_checkout(home).join(cockpit_config::PROJECT_CONFIG_FILE);
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(config, services_file(target)).unwrap();
    })
    .await
}

/// What a connection to the service inside the sandbox read first: `pong` once relayed,
/// nothing when the launcher closed it.
fn service_line() -> String {
    format!(
        "bash -c 'exec 3<>/dev/tcp/127.0.0.1/{SERVICE_LISTEN} && read -r -t 5 line <&3; \
         echo \"read=$line;\"'"
    )
}

fn is_relayed(printed: &str) -> bool {
    printed.contains("read=pong")
}

/// The agent's connection to the service once `is_done` accepts what it read: a decision
/// reaches the launcher within its poll.
async fn service_until(home: &Home, root: &Path, is_done: impl Fn(&str) -> bool) -> String {
    let deadline = std::time::Instant::now() + GENEROUS;
    loop {
        let printed = run_in_agent(home, root, &service_line()).await;
        if is_done(&printed) || std::time::Instant::now() > deadline {
            return printed;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Connects to the service once and waits until the owner's list shows its request, then
/// out of the grace in which an allow is refused (#156).
async fn service_asked_and_listed(home: &Home, agent: &Sandboxed, target: u16) {
    let printed = run_in_agent(home, &agent.root, &service_line()).await;
    assert!(!is_relayed(&printed), "{printed}");
    let asking = asking_line_for(&agent.session, &echo_record(target));
    let listed = network_list_once_it_contains(home, &asking).await;
    assert!(listed.contains(&asking), "{listed}");
    tokio::time::sleep(Duration::from_millis(2_100)).await;
}

/// #880: a declared service only asks. Its connection is closed, the owner's list shows
/// the whole record, the project file made no grant, and the evidence names the forward.
#[tokio::test(flavor = "multi_thread")]
async fn a_declared_service_is_closed_and_asked_of_the_owner_by_its_record() {
    let home = Home::new();
    let target = host_service().await;
    let agent = agent_declaring_echo(&home, target).await;

    service_asked_and_listed(&home, &agent, target).await;

    assert_eq!(owner_network(&home, &["grants"]).await, "");
    let evidence = session_sandbox(&home, &agent.session).await;
    assert!(
        evidence.contains(&format!("host-service forward: {}", echo_record(target))),
        "{evidence}"
    );
}

/// #880: "allow once" relays the session's next connection to the host's service.
#[tokio::test(flavor = "multi_thread")]
async fn allow_once_relays_the_next_connection_to_the_hosts_service() {
    let home = Home::new();
    let target = host_service().await;
    let agent = agent_declaring_echo(&home, target).await;
    service_asked_and_listed(&home, &agent, target).await;

    let allowed = owner_network(&home, &["allow", "1"]).await;
    let printed = service_until(&home, &agent.root, is_relayed).await;

    assert!(is_relayed(&printed), "{printed}");
    assert!(
        allowed.contains(&format!(
            "network request 1: {} for session {}",
            echo_record(target),
            agent.session
        )),
        "{allowed}"
    );
    assert!(
        allowed.contains("the service sees a local client"),
        "{allowed}"
    );
    assert_eq!(owner_network(&home, &["list"]).await, "");
}

/// #880: a project grant of the record relays the service until the owner revokes it; the
/// revocation closes the relay the agent holds open, and the next connection is closed.
#[tokio::test(flavor = "multi_thread")]
async fn a_revoked_service_grant_closes_the_open_relay_and_the_next_connection() {
    let home = Home::new();
    let target = host_service().await;
    let agent = agent_declaring_echo(&home, target).await;
    service_asked_and_listed(&home, &agent, target).await;
    owner_network(&home, &["allow", "1", "--always"]).await;
    let relayed = service_until(&home, &agent.root, is_relayed).await;
    assert!(is_relayed(&relayed), "{relayed}");
    let grants = owner_network(&home, &["grants"]).await;
    assert!(
        grants.contains(&format!(
            "grant\t1\t{COMMANDED_PROJECT}\t{}\trequest 1",
            echo_record(target)
        )),
        "{grants}"
    );

    let held = home.command_agent(
        &agent.root,
        &format!(
            "bash -c 'exec 3<>/dev/tcp/127.0.0.1/{SERVICE_LISTEN} && read -r -t 5 line <&3 && \
             echo $line > relay-open && timeout 60 cat <&3; echo \"closed=$?\"'"
        ),
    );
    let deadline = std::time::Instant::now() + GENEROUS;
    while !agent.root.join("relay-open").exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "the relay never opened"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    owner_network(&home, &["revoke", "1"]).await;
    let open_relay = home.agent_output(&agent.root, held).await;
    let next = service_until(&home, &agent.root, |printed| !is_relayed(printed)).await;

    assert!(open_relay.contains("closed=0"), "{open_relay}");
    assert!(!is_relayed(&next), "{next}");
}

/// #880, #71's audit M2: a project's grant reaches only its own sessions. Another project
/// declaring the same service is closed and asked of the owner for itself.
#[tokio::test(flavor = "multi_thread")]
async fn another_projects_session_does_not_reach_a_granted_service() {
    let home = Home::new();
    let target = host_service().await;
    let agent = agent_declaring_echo(&home, target).await;
    service_asked_and_listed(&home, &agent, target).await;
    owner_network(&home, &["allow", "1", "--always"]).await;
    let relayed = service_until(&home, &agent.root, is_relayed).await;
    assert!(is_relayed(&relayed), "{relayed}");
    let other_checkout = home.path().join("projects/other").join(COMMANDED_PROJECT);
    let config = other_checkout.join(cockpit_config::PROJECT_CONFIG_FILE);
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(config, services_file(target)).unwrap();
    let client = home.connect().await;
    let (other_root, other) = home
        .project_at(
            &client,
            &format!("other/{COMMANDED_PROJECT}"),
            Some("other"),
        )
        .await;
    let other_session = new_session(&client, &other_root).await;

    let printed = run_in_agent(&home, &other_root, &service_line()).await;

    assert!(!is_relayed(&printed), "{printed}");
    let asking = format!(
        "network\t2\tasking\t{}\t{other}\t{}",
        other_session.id,
        echo_record(target)
    );
    let listed = network_list_once_it_contains(&home, &asking).await;
    assert!(listed.contains(&asking), "{listed}");
}

/// #880: a service whose target is a host Unix socket answers at its `listen` path, in a
/// folder the session cannot write.
#[tokio::test(flavor = "multi_thread")]
async fn a_socket_service_answers_at_its_listen_path_once_allowed() {
    let home = Home::new();
    let target = home.path().join("host-echo.sock");
    let listener = std::os::unix::net::UnixListener::bind(&target).unwrap();
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        for mut stream in listener.incoming().flatten() {
            let mut request = [0; 1024];
            let _ = stream.read(&mut request);
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello");
        }
    });
    let record = format!(
        "service echo listen unix:/run/echo/echo.sock target unix:{}",
        target.display()
    );
    let agent = sandboxed_agent(&home, |home| {
        let config = commanded_checkout(home).join(cockpit_config::PROJECT_CONFIG_FILE);
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(
            config,
            format!(
                "[[services]]\nname = \"echo\"\nlisten = \"unix:/run/echo/echo.sock\"\n\
                 target = \"unix:{}\"\n",
                target.display()
            ),
        )
        .unwrap();
    })
    .await;
    let curl = "curl --silent --noproxy '*' --max-time 5 --unix-socket /run/echo/echo.sock \
                http://echo/; echo \";\"";
    run_in_agent(&home, &agent.root, curl).await;
    let asking = asking_line_for(&agent.session, &record);
    let listed = network_list_once_it_contains(&home, &asking).await;
    assert!(listed.contains(&asking), "{listed}");
    tokio::time::sleep(Duration::from_millis(2_100)).await;

    owner_network(&home, &["allow", "1"]).await;
    let deadline = std::time::Instant::now() + GENEROUS;
    let answered = loop {
        let printed = run_in_agent(&home, &agent.root, curl).await;
        if printed.starts_with("hello;") || std::time::Instant::now() > deadline {
            break printed;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let replaced = run_in_agent(
        &home,
        &agent.root,
        "rm -f /run/echo/echo.sock 2>/dev/null; echo rm=$?; touch /run/echo/other 2>/dev/null; \
         echo touch=$?",
    )
    .await;

    assert!(answered.starts_with("hello;"), "{answered}");
    assert_eq!(replaced, "rm=1\ntouch=1\nexit=0\n");
}

/// #880: the grant is bound to the whole record. A target edited in the project file is
/// another record: the project's next session is closed and the owner is asked again.
#[tokio::test(flavor = "multi_thread")]
async fn an_edited_target_is_asked_about_again_at_the_next_launch() {
    let home = Home::new();
    let target = host_service().await;
    let agent = agent_declaring_echo(&home, target).await;
    service_asked_and_listed(&home, &agent, target).await;
    owner_network(&home, &["allow", "1", "--always"]).await;
    let relayed = service_until(&home, &agent.root, is_relayed).await;
    assert!(is_relayed(&relayed), "{relayed}");

    let edited = host_service().await;
    let client = home.connect().await;
    let ended = client
        .request(
            Command::SessionEnd {
                session: agent.session.clone(),
            },
            GENEROUS,
        )
        .await;
    assert!(ended.is_ok(), "{ended:?}");
    std::fs::remove_dir_all(agent.root.join(".agent")).unwrap();
    std::fs::write(
        agent.root.join(cockpit_config::PROJECT_CONFIG_FILE),
        services_file(edited),
    )
    .unwrap();
    let next = new_session(&client, &agent.root).await;
    let printed = run_in_agent(&home, &agent.root, &service_line()).await;

    assert!(!is_relayed(&printed), "{printed}");
    let asking = format!(
        "network\t2\tasking\t{}\t{COMMANDED_PROJECT}\t{}",
        next.id,
        echo_record(edited)
    );
    let listed = network_list_once_it_contains(&home, &asking).await;
    assert!(listed.contains(&asking), "{listed}");
}

/// What the sandboxed agent's `torlek ask service SERVICE` printed, ending `exit=<code>`;
/// `service` may carry options after the name.
async fn ask_service_in_agent(home: &Home, root: &Path, service: &str) -> String {
    let torlek = candidate::program("torlek");
    run_in_agent(
        home,
        root,
        &format!(
            "'{}' ask service {service} --reason 'the tests read the echo' 2>&1",
            torlek.display()
        ),
    )
    .await
}

/// #880: `torlek ask service` asks ahead. The owner reads why in the list before deciding and
/// allows it, and the agent's first connection is relayed: none was closed to ask.
#[tokio::test(flavor = "multi_thread")]
async fn a_service_asked_ahead_relays_the_first_connection_once_allowed() {
    let home = Home::new();
    let target = host_service().await;
    let agent = agent_declaring_echo(&home, target).await;

    let asked = ask_service_in_agent(&home, &agent.root, "echo").await;

    assert!(
        asked.contains("network request 1") && asked.ends_with("exit=124\n"),
        "{asked}"
    );
    let listed = owner_network(&home, &["list"]).await;
    let asking = asking_line_for(&agent.session, &echo_record(target));
    assert!(
        listed.contains(&format!("{asking}\n  reason: the tests read the echo\n")),
        "{listed}"
    );
    tokio::time::sleep(Duration::from_millis(2_100)).await;
    owner_network(&home, &["allow", "1"]).await;
    let printed = service_until(&home, &agent.root, is_relayed).await;
    assert!(is_relayed(&printed), "{printed}");
}

/// #880: once allowed, asking again answers with the decision instead of a second request.
#[tokio::test(flavor = "multi_thread")]
async fn a_service_asked_again_once_allowed_exits_zero_without_a_second_request() {
    let home = Home::new();
    let target = host_service().await;
    let agent = agent_declaring_echo(&home, target).await;
    ask_service_in_agent(&home, &agent.root, "echo").await;
    tokio::time::sleep(Duration::from_millis(2_100)).await;
    owner_network(&home, &["allow", "1", "--always"]).await;

    let again = ask_service_in_agent(&home, &agent.root, "echo").await;

    assert!(
        again.contains("allowed: network grant 1") && again.ends_with("exit=0\n"),
        "{again}"
    );
    assert_eq!(owner_network(&home, &["list"]).await, "");
}

/// #880: only a service the project declares is asked for; the refusal names those it does.
#[tokio::test(flavor = "multi_thread")]
async fn an_ask_for_an_undeclared_service_is_refused_naming_the_declared_ones() {
    let home = Home::new();
    let target = host_service().await;
    let agent = agent_declaring_echo(&home, target).await;

    let refused = ask_service_in_agent(&home, &agent.root, "figma").await;

    assert!(
        refused.contains("no declared service figma; the project declares echo")
            && refused.ends_with("exit=1\n"),
        "{refused}"
    );
    assert_eq!(owner_network(&home, &["list"]).await, "");
}

/// #390: a container port is asked for only of the project's verified engine; until the owner
/// set it up and recorded it, the refusal names the owner's step and files nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_container_port_of_an_unverified_engine_is_refused_naming_the_owners_step() {
    let home = Home::new();
    let target = host_service().await;
    let agent = agent_declaring_echo(&home, target).await;

    let refused = ask_service_in_agent(&home, &agent.root, "containers --port 8080").await;

    assert!(
        refused.contains("is not verified, so no container port opens")
            && refused.contains("doctor --project")
            && refused.ends_with("exit=1\n"),
        "{refused}"
    );
    assert_eq!(owner_network(&home, &["list"]).await, "");
}

/// #390: the containers service opens a port the agent names; without one nothing is asked.
#[tokio::test(flavor = "multi_thread")]
async fn a_container_ask_without_its_port_is_refused() {
    let home = Home::new();
    let target = host_service().await;
    let agent = agent_declaring_echo(&home, target).await;

    let refused = ask_service_in_agent(&home, &agent.root, "containers").await;

    assert!(
        refused.contains("the containers service needs the port to open"),
        "{refused}"
    );
    assert_eq!(owner_network(&home, &["list"]).await, "");
}

/// #390: a declared service opens where the project declares it, never at a port named.
#[tokio::test(flavor = "multi_thread")]
async fn a_port_for_a_declared_service_is_refused() {
    let home = Home::new();
    let target = host_service().await;
    let agent = agent_declaring_echo(&home, target).await;

    let refused = ask_service_in_agent(&home, &agent.root, "echo --port 8080").await;

    assert!(
        refused.contains("--port names a container port; the declared service echo"),
        "{refused}"
    );
    assert_eq!(owner_network(&home, &["list"]).await, "");
}

/// #880: asking is an agent's request. The owner's socket refuses `ask service` and files
/// nothing.
#[tokio::test(flavor = "multi_thread")]
async fn the_owner_socket_refuses_an_ask_for_a_service() {
    let home = Home::new();
    let target = host_service().await;
    let _agent = agent_declaring_echo(&home, target).await;
    let owner = home.connect().await;

    let refused = owner
        .request(
            Command::AskService {
                launch_token: None,
                service: "echo".to_owned(),
                port: None,
                reason: "the owner tries".to_owned(),
            },
            GENEROUS,
        )
        .await;

    assert!(
        matches!(&refused, Err(ClientError::Daemon(error)) if error.code == ErrorCode::NotAuthorized),
        "{refused:?}"
    );
    assert_eq!(owner_network(&home, &["list"]).await, "");
}

/// What the agent sees of its compose project and port block (#1371), sorted.
const CONTAINER_PORT_VARIABLES: &str = "env | grep -E \
    '^(COMPOSE_PROJECT_NAME|TORLEK_PORT_(BASE|COUNT|0|9))=' | LC_ALL=C sort";

/// What [`CONTAINER_PORT_VARIABLES`] prints for `session` holding the block from `base`.
fn container_port_variables(session: &str, base: u16) -> String {
    format!(
        "COMPOSE_PROJECT_NAME={COMMANDED_PROJECT}-{session}\nTORLEK_PORT_0={base}\n\
         TORLEK_PORT_9={}\nTORLEK_PORT_BASE={base}\nTORLEK_PORT_COUNT=10\nexit=0\n",
        base + 9
    )
}

/// A second session of the [`COMMANDED_PROJECT`] in a clone of its own, whose directory's
/// name keeps the fake agent commanded; its working directory and id.
async fn clone_session(client: &Connection, root: &Path) -> (PathBuf, String) {
    let answer = client
        .request(
            Command::SessionNew {
                dir: root.display().to_string(),
                agent: Some("claude".to_owned()),
                prompt: None,
                clone: Some(COMMANDED_PROJECT.to_owned()),
                model: None,
                effort: None,
                compaction_threshold: None,
            },
            Duration::from_secs(60),
        )
        .await;
    let Ok(Reply::Session { session }) = answer else {
        panic!("session-new in a clone: {answer:?}");
    };
    assert_eq!(
        Path::new(&session.working_dir).file_name(),
        Some(std::ffi::OsStr::new(COMMANDED_PROJECT)),
        "{session:?}"
    );
    (PathBuf::from(&session.working_dir), session.id)
}

/// #1371: parallel sessions of one project run one compose file on its engine, each under a
/// compose project name and in a port block of its own.
#[tokio::test(flavor = "multi_thread")]
async fn parallel_sessions_of_one_project_get_their_own_compose_project_and_port_block() {
    let home = Home::new();
    let agent = sandboxed_agent(&home, |_| {}).await;
    let client = home.connect().await;
    let (clone, second) = clone_session(&client, &agent.root).await;

    let first_sees = run_in_agent(&home, &agent.root, CONTAINER_PORT_VARIABLES).await;
    let second_sees = run_in_agent(&home, &clone, CONTAINER_PORT_VARIABLES).await;

    assert_eq!(first_sees, container_port_variables(&agent.session, 20_000));
    assert_eq!(second_sees, container_port_variables(&second, 20_010));
}

/// #1371: `torlek session end` frees the session's block for the project's next session.
#[tokio::test(flavor = "multi_thread")]
async fn a_port_block_is_free_again_once_its_session_ends() {
    let home = Home::new();
    let agent = sandboxed_agent(&home, |_| {}).await;
    let client = home.connect().await;
    let (clone, second) = clone_session(&client, &agent.root).await;
    client
        .request(
            Command::SessionEnd {
                session: agent.session.clone(),
            },
            GENEROUS,
        )
        .await
        .expect("the owner ends the session");

    let third = new_session(&client, &agent.root).await;

    let third_sees = run_in_agent(&home, &agent.root, CONTAINER_PORT_VARIABLES).await;
    let second_sees = run_in_agent(&home, &clone, CONTAINER_PORT_VARIABLES).await;
    assert_eq!(third_sees, container_port_variables(&third.id, 20_000));
    assert_eq!(second_sees, container_port_variables(&second, 20_010));
}

/// #1371: a container port in another session's block is that session's; asking for it is
/// refused naming the session, and nothing is filed.
#[tokio::test(flavor = "multi_thread")]
async fn a_container_port_in_another_sessions_block_is_refused_naming_that_session() {
    let home = Home::new();
    let agent = sandboxed_agent(&home, |_| {}).await;
    let client = home.connect().await;
    let (clone, _) = clone_session(&client, &agent.root).await;

    let refused = ask_service_in_agent(&home, &clone, "containers --port 20005").await;

    assert!(
        refused.contains(&format!(
            "port 20005 is in session {}'s port block (20000-20009)",
            agent.session
        )) && refused.ends_with("exit=1\n"),
        "{refused}"
    );
    assert_eq!(owner_network(&home, &["list"]).await, "");
}

/// #1371: `doctor --project` names the line of a literal host port and the variable that
/// lets parallel sessions publish it.
#[tokio::test(flavor = "multi_thread")]
async fn doctor_names_a_compose_files_literal_host_port_and_its_fix() {
    let home = Home::new();
    home.write_owner_config(REQUIRED);
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, "app").await;
    std::fs::write(
        root.join("compose.yaml"),
        "services:\n  web:\n    image: nginx\n    ports:\n      - \"8080:80\"\n",
    )
    .unwrap();

    let doctor = home
        .cockpit(&["doctor", "--project", root.to_str().unwrap()], &[])
        .await;

    let report = text(&doctor.stdout);
    let line = report
        .lines()
        .find(|line| line.contains("compose compose.yaml:5: "))
        .unwrap_or_else(|| panic!("no compose line in:\n{report}"));
    assert!(line.contains("literal host port 8080"), "{report}");
    assert!(report.contains("${TORLEK_PORT_0:-8080}"), "{report}");
}

/// #1371: an agent writes the checkout, so doctor never follows a compose file that is a
/// symbolic link.
#[tokio::test(flavor = "multi_thread")]
async fn doctor_does_not_follow_a_compose_file_that_is_a_symbolic_link() {
    let home = Home::new();
    home.write_owner_config(REQUIRED);
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, "app").await;
    let outside = home.path().join("outside.yaml");
    std::fs::write(
        &outside,
        "services:\n  web:\n    ports:\n      - \"8080:80\"\n",
    )
    .unwrap();
    std::os::unix::fs::symlink(&outside, root.join("compose.yaml")).unwrap();

    let doctor = home
        .cockpit(&["doctor", "--project", root.to_str().unwrap()], &[])
        .await;

    let report = text(&doctor.stdout);
    let line = report
        .lines()
        .find(|line| line.contains("compose compose.yaml: "))
        .unwrap_or_else(|| panic!("no compose line in:\n{report}"));
    assert!(
        line.starts_with("unknown ") && line.contains("symbolic link"),
        "{report}"
    );
    assert!(!report.contains("8080"), "{report}");
}

/// #1371: an unsandboxed agent uses the owner's own engine, where Torlek hands out no ports.
#[tokio::test(flavor = "multi_thread")]
async fn an_unsandboxed_session_launches_with_no_compose_project_or_port_block() {
    let home = Home::new();
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, COMMANDED_PROJECT).await;
    new_session(&client, &root).await;

    let sees = run_in_agent(&home, &root, CONTAINER_PORT_VARIABLES).await;

    // Nothing printed; the exit is `sort`'s.
    assert_eq!(sees, "exit=0\n");
}

/// #1371: a session keeps its block across relaunches, so its running containers keep
/// their ports.
#[tokio::test(flavor = "multi_thread")]
async fn a_restarted_session_keeps_its_port_block() {
    let home = Home::new();
    let agent = sandboxed_agent(&home, |_| {}).await;
    let client = home.connect().await;
    clone_session(&client, &agent.root).await;
    restart(&home, &agent.session).await.unwrap();
    let launch = listed(&client, &agent.session).await.launch;
    assert_eq!(
        launch.map(|launch| launch.kind),
        Some(LaunchKindView::Restart)
    );

    let sees = run_in_agent(&home, &agent.root, CONTAINER_PORT_VARIABLES).await;

    assert_eq!(sees, container_port_variables(&agent.session, 20_000));
}
