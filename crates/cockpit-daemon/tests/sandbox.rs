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
use std::time::Duration;

use cockpit_client::{ClientError, Connection};
use cockpit_protocol::{Command, ErrorCode};
use real_daemon::{
    BUILD_PROFILE_DIR, COMMANDED_PROJECT, Daemon, GENEROUS, Home, SETTLE, new_session,
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

#[tokio::test(flavor = "multi_thread")]
async fn cockpit_sandbox_show_prints_no_sandbox_and_the_bare_command_without_launching() {
    let home = Home::new();
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, "app").await;

    let shown = sandbox_show(&home, &root).await;

    assert!(shown.status.success(), "{}", text(&shown.stderr));
    let printed = text(&shown.stdout);
    assert!(printed.starts_with("sandbox: NONE on "), "{printed}");
    assert!(printed.contains("\ncommand: <agent>\n"), "{printed}");
    assert!(home.launches().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn cockpit_sandbox_show_with_a_required_setting_without_bubblewrap_fails_naming_why() {
    let home = Home::new();
    home.write_owner_config(REQUIRED);
    let _daemon = home.start_without_bubblewrap();
    let client = home.connect().await;
    let (root, _) = home.project(&client, "app").await;

    let shown = sandbox_show(&home, &root).await;

    assert!(!shown.status.success());
    assert!(
        text(&shown.stderr).contains(NO_BUBBLEWRAP),
        "{}",
        text(&shown.stderr)
    );
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
                .map(|(variable, _)| *variable),
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
                 session may not reach it"
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

/// #856: no container engine socket, nor the variables that point at one.
#[tokio::test(flavor = "multi_thread")]
async fn a_sandboxed_agent_reaches_no_container_engine() {
    let home = Home::new();
    let agent = sandboxed_agent(&home, |_| {}).await;

    let printed = run_in_agent(
        &home,
        &agent.root,
        "for socket in /var/run/docker.sock /run/docker.sock /run/podman/podman.sock \
         /run/containerd/containerd.sock \"${XDG_RUNTIME_DIR-/nonexistent}/docker.sock\" \
         \"${XDG_RUNTIME_DIR-/nonexistent}/podman/podman.sock\"; do \
         test -e \"$socket\" && echo \"present: $socket\"; done; \
         env | grep '^DOCKER_' || echo 'no docker variables'",
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

    assert!(
        printed.starts_with("HTTP/1.1 403 Forbidden\r\n"),
        "{printed}"
    );
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
            printed.starts_with("HTTP/1.1 403 Forbidden\r\n"),
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
    format!("network\t1\tasking\t{session}\t{COMMANDED_PROJECT}\t{ASKED_HOST}")
}

/// Refuses [`ASKED_HOST`] once and waits until the owner's list shows the request, then out
/// of the grace in which an allow is refused (#156).
async fn asked_and_listed(home: &Home, agent: &Sandboxed) {
    run_in_agent(home, &agent.root, &connect_line(ASKED_HOST)).await;
    let listed = network_list_once_it_contains(home, &asking_line(&agent.session)).await;
    assert!(listed.contains(&asking_line(&agent.session)), "{listed}");
    tokio::time::sleep(Duration::from_millis(2_100)).await;
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
    printed.starts_with("HTTP/1.1 403 Forbidden\r\n")
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
