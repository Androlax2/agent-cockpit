//! `cockpit doctor` against the real daemon binary (#50): the readiness report names each
//! requirement with its status and the next action, from the hosts the daemon found at its
//! start (stand-in claude and `docker`), and exits 1
//! only when a requirement blocks.

mod candidate;
mod real_daemon;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use cockpit_client::Connection;
use cockpit_protocol::{Command, ErrorCode, Reply};
use real_daemon::{GENEROUS, Home, new_session};

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// The line `doctor` printed for `name`, and the `next:` line under it when there is one.
fn requirement<'report>(report: &'report str, name: &str) -> (&'report str, Option<&'report str>) {
    let mut lines = report.lines();
    let line = lines
        .by_ref()
        .find(|line| line.contains(&format!("  {name}: ")))
        .unwrap_or_else(|| panic!("no {name} requirement in:\n{report}"));
    let next = lines
        .next()
        .and_then(|line| line.trim_start().strip_prefix("next: "));
    (line, next)
}

fn status(line: &str) -> &str {
    line.split_whitespace().next().unwrap()
}

fn fake_docker(home: &Home, info: &str) {
    let program = home.path().join("bin/docker");
    std::fs::write(
        &program,
        format!("#!/bin/sh\n[ \"$1\" = info ] || exit 9\nprintf '%s\\n' '{info}'\n"),
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// A `name` on the daemon's PATH that fails every invocation, its `--version` included.
fn fake_failing_program(home: &Home, name: &str) {
    let program = home.path().join("bin").join(name);
    std::fs::write(&program, "#!/bin/sh\nexit 1\n").unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn declare_docker(root: &Path) {
    std::fs::create_dir_all(root.join(".torlek")).unwrap();
    std::fs::write(
        root.join(".torlek/config.toml"),
        "[execution]\nenvironment = \"docker\"\n",
    )
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn doctor_reports_each_sign_in_answer_and_refreshes_without_a_restart() {
    let home = Home::new();
    let program = home.path().join("bin/claude");
    std::fs::write(
        &program,
        r#"#!/bin/sh
if [ "$1" = --version ]; then echo '2.1.316 (Claude Code)'; exit 0; fi
[ "$*" = 'auth status' ] || exit 99
cat "$HOME/sign-in.json"
exit "$(cat "$HOME/sign-in.exit")"
"#,
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let _daemon = home.start();

    for (answer, exit, expected, next_command, onboarding) in [
        (
            r#"{"loggedIn":true,"email":"private@example.invalid"}"#,
            "0",
            "available",
            None,
            "done",
        ),
        (
            r#"{"loggedIn":false}"#,
            "1",
            "needs-authorization",
            Some("`/login`"),
            "todo",
        ),
        (
            "private malformed output",
            "2",
            "unknown",
            Some("claude auth status"),
            "unknown",
        ),
        (r#"{"loggedIn":true}"#, "0", "available", None, "done"),
    ] {
        std::fs::write(home.path().join("sign-in.json"), answer).unwrap();
        std::fs::write(home.path().join("sign-in.exit"), exit).unwrap();
        let doctor = home.cockpit(&["doctor"], &[]).await;
        let report = text(&doctor.stdout);
        assert!(doctor.status.success(), "{report}{}", text(&doctor.stderr));
        let (line, next) = requirement(&report, "provider claude sign-in");
        assert_eq!(status(line), expected, "{report}");
        match next_command {
            Some(command) => assert!(next.unwrap().contains(command), "{report}"),
            None => assert!(next.is_none(), "{report}"),
        }
        assert!(!report.contains("private"), "{report}");
        let init = home.cockpit(&["init"], &[]).await;
        let checklist = text(&init.stdout);
        let sign_in_step: Vec<&str> = checklist
            .lines()
            .skip_while(|line| !line.contains("Sign in"))
            .take_while(|line| !line.is_empty())
            .collect();
        assert!(
            sign_in_step
                .first()
                .is_some_and(|line| line.contains(onboarding)),
            "{checklist}"
        );
        if let Some(command) = next_command {
            assert!(
                sign_in_step
                    .iter()
                    .any(|line| line.trim_start().starts_with("next: ") && line.contains(command)),
                "{checklist}"
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_test_home_runs_with_the_disk_guard_off() {
    let home = Home::new();
    let _daemon = home.start();

    let doctor = home.cockpit(&["doctor"], &[]).await;

    let report = text(&doctor.stdout);
    assert!(!report.contains("  disk: "), "{report}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_test_that_sets_a_floor_turns_the_disk_guard_on() {
    let home = Home::new();
    home.write_owner_config("[storage]\nlow_disk_floor_mib = 1000000000\n");
    let _daemon = home.start();

    let doctor = home.cockpit(&["doctor"], &[]).await;

    let report = text(&doctor.stdout);
    assert_eq!(doctor.status.code(), Some(1), "{report}");
    let (line, _) = requirement(&report, "disk");
    assert_eq!(status(line), "missing-prerequisite", "{line}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_host_with_its_programs_reports_them_available_and_exits_0() {
    let home = Home::new();
    let _daemon = home.start();

    let doctor = home.cockpit(&["doctor"], &[]).await;

    let report = text(&doctor.stdout);
    assert!(doctor.status.success(), "{report}{}", text(&doctor.stderr));
    for name in [
        "config",
        "platform",
        "tmux",
        "sandbox",
        "provider claude",
        "resource controls",
    ] {
        let (line, _) = requirement(&report, name);
        assert_eq!(status(line), "available", "{line}");
    }
    let (sandbox, _) = requirement(&report, "sandbox");
    assert!(sandbox.contains("no sandbox: "), "{sandbox}");
    let (sign_in, next) = requirement(&report, "provider claude sign-in");
    assert_eq!(status(sign_in), "unknown", "{sign_in}");
    assert!(next.is_some(), "{report}");
}

/// #411: one usable provider is enough; one that does not run is listed as optional and the
/// report still passes. #623: it is already on PATH, so the advice is not to install it.
#[tokio::test(flavor = "multi_thread")]
async fn a_provider_that_does_not_run_is_listed_as_optional_and_exits_0() {
    let home = Home::new();
    fake_failing_program(&home, "codex");
    let _daemon = home.start();

    let doctor = home.cockpit(&["doctor"], &[]).await;

    let report = text(&doctor.stdout);
    assert!(doctor.status.success(), "{report}{}", text(&doctor.stderr));
    let (line, next) = requirement(&report, "provider codex");
    assert!(line.contains("not usable, optional"), "{line}");
    let next = next.unwrap_or_else(|| panic!("no next action in:\n{report}"));
    assert!(next.contains("`codex --version`"), "{next}");
    assert!(!next.contains("install"), "{next}");
}

/// #848: CMD+V in the viewer reads the clipboard with `wl-paste` on the display the owner
/// imported; doctor says it works once both are there.
#[tokio::test(flavor = "multi_thread")]
async fn image_paste_is_available_with_wl_paste_and_the_owners_wayland_display() {
    let home = Home::new();
    fake_failing_program(&home, "wl-paste");
    let _daemon = home.start_with(&[("WAYLAND_DISPLAY", "wayland-test")]);

    let doctor = home.cockpit(&["doctor"], &[]).await;

    let report = text(&doctor.stdout);
    let (line, next) = requirement(&report, "clipboard in");
    assert_eq!(status(line), "available", "{report}");
    assert!(line.contains("CMD+V"), "{line}");
    assert!(next.is_none(), "{report}");
}

#[tokio::test(flavor = "multi_thread")]
async fn without_the_owners_wayland_display_image_paste_is_optional_naming_the_import() {
    let home = Home::new();
    fake_failing_program(&home, "wl-paste");
    let _daemon = home.start();

    let doctor = home.cockpit(&["doctor"], &[]).await;

    let report = text(&doctor.stdout);
    assert!(doctor.status.success(), "{report}{}", text(&doctor.stderr));
    let (line, next) = requirement(&report, "clipboard in");
    assert!(line.contains("not usable, optional"), "{line}");
    let next = next.unwrap_or_else(|| panic!("no next action in:\n{report}"));
    assert!(line.contains("no Wayland display imported"), "{line}");
    assert!(next.contains("torlek desktop-session import"), "{next}");
}

/// Copy out depends on the tmux the daemon found (`get-clipboard`, which 3.6 lacks), so the
/// line says which, and never blocks.
#[tokio::test(flavor = "multi_thread")]
async fn clipboard_out_is_listed_without_blocking() {
    let home = Home::new();
    let _daemon = home.start();

    let doctor = home.cockpit(&["doctor"], &[]).await;

    let report = text(&doctor.stdout);
    assert!(doctor.status.success(), "{report}{}", text(&doctor.stderr));
    let (line, _) = requirement(&report, "clipboard out");
    assert_eq!(status(line), "available", "{line}");
    assert!(
        line.contains("OSC 52") || line.contains("not usable, optional"),
        "{line}"
    );
}

/// #624: init's provider step reads the report doctor prints, so a provider doctor calls not
/// usable is not offered to sessions.
#[tokio::test(flavor = "multi_thread")]
async fn init_offers_only_the_providers_doctor_finds_usable() {
    let home = Home::new();
    fake_failing_program(&home, "codex");
    let _daemon = home.start();

    let init = home.cockpit(&["init"], &[]).await;

    let checklist = text(&init.stdout);
    assert!(init.status.success(), "{checklist}{}", text(&init.stderr));
    let provider_step: Vec<&str> = checklist
        .lines()
        .skip_while(|line| !line.ends_with(". Pick a provider"))
        .take_while(|line| !line.is_empty())
        .collect();
    let offered = provider_step
        .get(1)
        .unwrap_or_else(|| panic!("no offered provider in:\n{checklist}"));
    assert!(offered.contains("claude"), "{offered}");
    assert!(!offered.contains("codex"), "{offered}");
    assert!(
        provider_step
            .iter()
            .any(|line| line.contains("provider codex: not usable")),
        "{checklist}"
    );
}

/// #828, #829: on a host without the sandbox (no `bwrap` on the daemon's PATH), `required`
/// blocks and names why rather than run unsandboxed.
#[tokio::test(flavor = "multi_thread")]
async fn a_required_sandbox_without_bubblewrap_blocks_doctor_and_init_naming_why() {
    let home = Home::new();
    home.write_owner_config("[agents]\nsandbox = \"required\"\n");
    let _daemon = home.start_without_bubblewrap();

    let doctor = home.cockpit(&["doctor"], &[]).await;

    let report = text(&doctor.stdout);
    assert_eq!(doctor.status.code(), Some(1), "{report}");
    let (sandbox, next) = requirement(&report, "sandbox");
    assert_eq!(status(sandbox), "missing-prerequisite", "{sandbox}");
    assert!(sandbox.contains("bubblewrap (`bwrap`)"), "{sandbox}");
    assert!(
        next.unwrap().contains("agents.sandbox = \"off\""),
        "{report}"
    );
    let init = home.cockpit(&["init"], &[]).await;
    assert!(
        text(&init.stdout).contains("bubblewrap (`bwrap`)"),
        "{}",
        text(&init.stdout)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_default_unsandboxed_setting_is_reported_loudly_without_blocking() {
    let home = Home::new();
    let _daemon = home.start();

    let doctor = home.cockpit(&["doctor"], &[]).await;

    let report = text(&doctor.stdout);
    assert!(doctor.status.success(), "{report}");
    let (sandbox, next) = requirement(&report, "sandbox");
    assert_eq!(status(sandbox), "available", "{sandbox}");
    assert!(sandbox.contains("UNSANDBOXED"), "{sandbox}");
    assert!(next.unwrap().contains("agents.sandbox"), "{report}");
}

/// #963: the daemon holds `agents.sandbox` from its last read of the owner's file. A value
/// written since is not in force, and `doctor` says so with the command that applies it rather
/// than advise the owner to write what they just wrote.
#[tokio::test(flavor = "multi_thread")]
async fn a_sandbox_setting_written_while_the_daemon_runs_blocks_doctor_naming_the_reload() {
    let home = Home::new();
    let _daemon = home.start();
    home.write_owner_config("[agents]\nsandbox = \"required\"\n");

    let doctor = home.cockpit(&["doctor"], &[]).await;

    let report = text(&doctor.stdout);
    assert_eq!(doctor.status.code(), Some(1), "{report}");
    let (sandbox, next) = requirement(&report, "sandbox");
    assert_eq!(status(sandbox), "missing-prerequisite", "{sandbox}");
    assert!(
        sandbox.contains("agents.sandbox = \"required\"")
            && sandbox.contains("config.toml:2")
            && sandbox.contains("the running daemon still applies \"off\"")
            && sandbox.contains("UNSANDBOXED"),
        "{sandbox}"
    );
    let next = next.unwrap();
    assert!(
        next.contains("`torlek config reload`") && next.contains("restarting the daemon"),
        "{next}"
    );
}

/// #963: the reload `doctor` named brings the written value into force, and `doctor` reports
/// that value alone. The daemon starts without `bwrap`, so `required` refuses every launch.
#[tokio::test(flavor = "multi_thread")]
async fn after_the_reload_doctor_reports_the_written_sandbox_setting_as_in_force() {
    let home = Home::new();
    home.write_owner_config("[agents]\nsandbox = \"required\"\n");
    let _daemon = home.start_without_bubblewrap();
    home.write_owner_config("[agents]\nsandbox = \"off\"\n");
    let stale = home.cockpit(&["doctor"], &[]).await;
    let reloaded = home.cockpit(&["config", "reload"], &[]).await;
    assert!(reloaded.status.success(), "{}", text(&reloaded.stderr));

    let doctor = home.cockpit(&["doctor"], &[]).await;

    let stale = text(&stale.stdout);
    let (stale_sandbox, _) = requirement(&stale, "sandbox");
    assert!(
        stale_sandbox.contains("the running daemon still applies \"required\"")
            && stale_sandbox.contains("every agent launch is still refused"),
        "{stale_sandbox}"
    );
    let report = text(&doctor.stdout);
    assert!(doctor.status.success(), "{report}");
    let (sandbox, _) = requirement(&report, "sandbox");
    assert_eq!(status(sandbox), "available", "{sandbox}");
    assert!(
        sandbox.contains("agents run UNSANDBOXED") && !sandbox.contains("still applies"),
        "{sandbox}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unregistered_directory_names_the_project_add_command() {
    let home = Home::new();
    let _daemon = home.start();
    let dir = home.path().join("elsewhere");
    std::fs::create_dir_all(&dir).unwrap();

    let doctor = home
        .cockpit(&["doctor", "--project", dir.to_str().unwrap()], &[])
        .await;

    let report = text(&doctor.stdout);
    assert_eq!(doctor.status.code(), Some(1), "{report}");
    let (project, next) = requirement(&report, "project");
    assert_eq!(status(project), "missing-prerequisite", "{project}");
    assert!(
        next.unwrap()
            .contains(&format!("torlek project add {}", dir.display())),
        "{report}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_untrusted_project_needs_authorization_without_blocking() {
    let home = Home::new();
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, "app").await;

    let doctor = home
        .cockpit(&["doctor", "--project", root.to_str().unwrap()], &[])
        .await;

    let report = text(&doctor.stdout);
    assert!(doctor.status.success(), "{report}");
    let (project, _) = requirement(&report, "project");
    assert_eq!(status(project), "available", "{project}");
    let (trust, next) = requirement(&report, "project trust");
    assert_eq!(status(trust), "needs-authorization", "{trust}");
    assert!(next.unwrap().contains("projects.trusted"), "{report}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_docker_project_reports_the_rootless_engine_it_reaches() {
    let home = Home::new();
    fake_docker(
        &home,
        r#"{"ID":"7c2b","Name":"box","ServerVersion":"28.1.1","SecurityOptions":["name=rootless"]}"#,
    );
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, "app").await;
    declare_docker(&root);

    let doctor = home
        .cockpit(&["doctor", "--project", root.to_str().unwrap()], &[])
        .await;

    let report = text(&doctor.stdout);
    assert!(doctor.status.success(), "{report}");
    let (docker, _) = requirement(&report, "docker");
    assert_eq!(status(docker), "available", "{docker}");
    assert!(
        docker.contains("engine 7c2b on box (Docker 28.1.1), rootless"),
        "{docker}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_docker_project_on_a_rootful_engine_blocks() {
    let home = Home::new();
    fake_docker(
        &home,
        r#"{"ID":"7c2b","Name":"box","ServerVersion":"28.1.1","SecurityOptions":["name=seccomp,profile=builtin"]}"#,
    );
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, "app").await;
    declare_docker(&root);

    let doctor = home
        .cockpit(&["doctor", "--project", root.to_str().unwrap()], &[])
        .await;

    let report = text(&doctor.stdout);
    assert_eq!(doctor.status.code(), Some(1), "{report}");
    let (docker, next) = requirement(&report, "docker");
    assert_eq!(status(docker), "missing-prerequisite", "{docker}");
    assert!(next.unwrap().contains("rootless"), "{report}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_project_that_does_not_need_docker_never_runs_it() {
    let home = Home::new();
    let probed = home.path().join("docker-ran");
    let program = home.path().join("bin/docker");
    std::fs::write(
        &program,
        format!("#!/bin/sh\ntouch '{}'\n", probed.display()),
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, "app").await;

    let doctor = home
        .cockpit(&["doctor", "--project", root.to_str().unwrap()], &[])
        .await;

    let report = text(&doctor.stdout);
    assert!(!report.contains("  docker: "), "{report}");
    assert!(!probed.exists());
}

/// #390, #856's table: sandboxed sessions never reach the owner's engine, so under
/// `required` a Docker project needs its own, and doctor writes the script that sets it up.
#[tokio::test(flavor = "multi_thread")]
async fn a_sandboxed_docker_project_without_its_engine_gets_the_setup_script() {
    let home = Home::new();
    home.write_owner_config("[agents]\nsandbox = \"required\"\n");
    let probed = home.path().join("docker-ran");
    let program = home.path().join("bin/docker");
    std::fs::write(
        &program,
        format!("#!/bin/sh\ntouch '{}'\n", probed.display()),
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, "app").await;
    declare_docker(&root);

    let doctor = home
        .cockpit(&["doctor", "--project", root.to_str().unwrap()], &[])
        .await;

    let report = text(&doctor.stdout);
    assert_eq!(doctor.status.code(), Some(1), "{report}");
    let (docker, next) = requirement(&report, "docker");
    assert_eq!(status(docker), "missing-prerequisite", "{docker}");
    assert!(
        docker.contains("no Torlek-run engine for this project"),
        "{docker}"
    );
    let script = home.state_dir().join("containers/torlek-dev-e1-setup.sh");
    let next = next.unwrap();
    assert!(
        next.contains(&format!("`sudo sh {}`", script.display()))
            && next.contains("`torlek-dev containers record app`"),
        "{next}"
    );
    let written = std::fs::read_to_string(&script).unwrap();
    assert!(written.contains("user='torlek-dev-e1'"), "{written}");
    assert!(!probed.exists(), "the owner's own docker was asked");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_relative_project_directory_is_refused_on_the_wire() {
    let home = Home::new();
    let _daemon = home.start();
    let client = home.connect().await;

    let refused = client
        .request(
            Command::Readiness {
                project: Some("projects/app".to_owned()),
                is_one_sign_in_enough: false,
            },
            GENEROUS,
        )
        .await;

    let error = real_daemon_refusal(refused);
    assert_eq!(error.code, ErrorCode::InvalidArgument);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_may_not_ask_for_the_owners_readiness_report() {
    let home = Home::new();
    let _daemon = home.start();

    let agent = Connection::open(&home.agent_socket_path(), GENEROUS)
        .await
        .expect("the agent socket accepts the handshake");
    let refused = agent
        .request(
            Command::Readiness {
                project: None,
                is_one_sign_in_enough: false,
            },
            GENEROUS,
        )
        .await;

    let error = real_daemon_refusal(refused);
    assert_eq!(error.code, ErrorCode::NotAuthorized, "{error:?}");
}

fn real_daemon_refusal(
    reply: Result<Reply, cockpit_client::ClientError>,
) -> cockpit_protocol::ProtocolError {
    match reply {
        Err(cockpit_client::ClientError::Daemon(error)) => error,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_project_directory_that_does_not_exist_is_refused_naming_it() {
    let home = Home::new();
    let _daemon = home.start();
    let client = home.connect().await;
    let missing = home.path().join("typo");

    let refused = client
        .request(
            Command::Readiness {
                project: Some(missing.display().to_string()),
                is_one_sign_in_enough: false,
            },
            GENEROUS,
        )
        .await;

    let error = real_daemon_refusal(refused);
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert!(error.message.contains("typo"), "{error:?}");
}

const CODEX_SIGNED_IN: &str = "echo 'Logged in using ChatGPT' >&2";
const CODEX_SIGNED_OUT: &str = "echo 'Not logged in' >&2; exit 1";
const OPENCODE_SIGNED_IN: &str = "printf '└  1 credentials\\n'";
const OPENCODE_SIGNED_OUT: &str = "printf '└  0 credentials\\n'";

/// `name` on the daemon's PATH: `--version` answers the adapter's conformance version and
/// its sign-in status command runs `answer`, a line of shell.
fn fake_sign_in(home: &Home, name: &str, answer: &str) {
    let (version, status_command) = match name {
        "codex" => ("codex-cli 0.166.0", "login status"),
        "opencode" => ("1.19.5", "auth list"),
        other => panic!("no fake sign-in for {other}"),
    };
    let program = home.path().join("bin").join(name);
    std::fs::write(
        &program,
        format!(
            "#!/bin/sh\nif [ \"$1\" = --version ]; then echo '{version}'; exit 0; fi\n\
             [ \"$*\" = '{status_command}' ] || exit 99\n{answer}\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// `answer` once `seconds` have passed, leaving `marker` in the home when it does.
fn slowly(seconds: &str, marker: &str, answer: &str) -> String {
    format!("sleep {seconds}; : > \"$HOME/{marker}\"; {answer}")
}

/// A daemon whose owner did every first-run step but the sign-in: the sandbox consent, a
/// project and a session.
async fn start_with_the_setup_done(home: &Home) -> real_daemon::Daemon {
    home.write_owner_config("[agents]\nsandbox = \"off\"\n");
    let daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, "app").await;
    new_session(&client, &root).await;
    daemon
}

/// `torlek` alone, reading the owner's configuration where the daemon reads it.
async fn torlek_alone(home: &Home) -> std::process::Output {
    let config_home = home.path().join("config");
    home.cockpit(&[], &[("XDG_CONFIG_HOME", config_home.to_str().unwrap())])
        .await
}

/// The lines of the checklist's step `title`, its heading first.
fn checklist_step<'checklist>(checklist: &'checklist str, title: &str) -> Vec<&'checklist str> {
    let step: Vec<&str> = checklist
        .lines()
        .skip_while(|line| !line.ends_with(title))
        .take_while(|line| !line.is_empty())
        .collect();
    assert!(!step.is_empty(), "no {title} step in:\n{checklist}");
    step
}

/// #1048: each provider's sign-in command answers only once the other's has started, so a
/// daemon that asked them one after the other would read the first as unknown.
#[tokio::test(flavor = "multi_thread")]
async fn doctor_asks_the_providers_for_their_sign_in_at_the_same_time() {
    let home = Home::new();
    let once_the_other_is_asked = |own: &str, other: &str, answer: &str| {
        format!(
            ": > \"$HOME/{own}-asked\"\nfor attempt in $(seq 100); do\n\
             if [ -e \"$HOME/{other}-asked\" ]; then {answer}; exit 0; fi\nsleep 0.05\ndone\nexit 3"
        )
    };
    fake_sign_in(
        &home,
        "codex",
        &once_the_other_is_asked("codex", "opencode", CODEX_SIGNED_IN),
    );
    fake_sign_in(
        &home,
        "opencode",
        &once_the_other_is_asked("opencode", "codex", OPENCODE_SIGNED_IN),
    );
    let _daemon = home.start();

    let doctor = home.cockpit(&["doctor"], &[]).await;

    let report = text(&doctor.stdout);
    for name in ["provider codex sign-in", "provider opencode sign-in"] {
        let (line, _) = requirement(&report, name);
        assert_eq!(status(line), "available", "{report}");
    }
}

/// #1048: the usage needs one provider signed in, so `torlek` alone answers when the first
/// one says so; opencode's `auth list` takes most of a second on a real host.
#[tokio::test(flavor = "multi_thread")]
async fn torlek_alone_prints_the_usage_without_waiting_for_a_slower_providers_sign_in() {
    let home = Home::new();
    fake_sign_in(&home, "codex", CODEX_SIGNED_IN);
    fake_sign_in(
        &home,
        "opencode",
        &slowly("5", "opencode-answered", OPENCODE_SIGNED_IN),
    );
    let _daemon = start_with_the_setup_done(&home).await;

    let alone = torlek_alone(&home).await;

    let has_opencode_answered = home.path().join("opencode-answered").exists();
    let usage = text(&alone.stderr);
    assert_eq!(alone.status.code(), Some(2), "{usage}");
    assert!(usage.starts_with("usage: torlek "), "{usage}");
    assert!(
        !has_opencode_answered,
        "torlek alone waited for the slower provider's sign-in"
    );
}

/// #1048: a status command an early answer left running is outside the bulk lane, so only
/// one report's may be: the next `torlek` alone waits for its own instead of adding to them.
#[tokio::test(flavor = "multi_thread")]
async fn a_second_torlek_alone_waits_while_the_firsts_status_command_still_runs() {
    let home = Home::new();
    fake_sign_in(&home, "codex", CODEX_SIGNED_IN);
    fake_sign_in(
        &home,
        "opencode",
        &format!("sleep 2; echo answered >> \"$HOME/opencode-answers\"; {OPENCODE_SIGNED_IN}"),
    );
    let _daemon = start_with_the_setup_done(&home).await;
    let answers = || {
        std::fs::read_to_string(home.path().join("opencode-answers"))
            .map_or(0, |answers| answers.lines().count())
    };

    torlek_alone(&home).await;
    let answers_after_the_first = answers();
    let second = torlek_alone(&home).await;

    assert_eq!(answers_after_the_first, 0);
    assert_eq!(second.status.code(), Some(2), "{}", text(&second.stderr));
    assert_eq!(answers(), 2);
}

/// #1048: answering early never hides a missing sign-in: with no provider signed in, the
/// slower provider is waited for and the step is open.
#[tokio::test(flavor = "multi_thread")]
async fn torlek_alone_opens_the_sign_in_step_when_no_provider_is_signed_in() {
    let home = Home::new();
    fake_sign_in(&home, "codex", CODEX_SIGNED_OUT);
    fake_sign_in(
        &home,
        "opencode",
        &slowly("0.3", "opencode-answered", OPENCODE_SIGNED_OUT),
    );
    let _daemon = start_with_the_setup_done(&home).await;

    let alone = torlek_alone(&home).await;

    let checklist = text(&alone.stdout);
    assert!(alone.status.success(), "{checklist}{}", text(&alone.stderr));
    let sign_in = checklist_step(&checklist, ". Sign in to the provider");
    assert!(sign_in[0].starts_with("[todo"), "{checklist}");
    for provider in ["codex", "opencode"] {
        assert!(
            sign_in.iter().any(|line| line
                .trim_start()
                .starts_with(&format!("provider {provider} sign-in: not signed in"))),
            "{checklist}"
        );
    }
}

/// #1048: the checklist names each provider's own sign-in, as `torlek init` does, also for a
/// provider that answers after the first signed-in one.
#[tokio::test(flavor = "multi_thread")]
async fn torlek_alone_with_a_step_open_lists_the_sign_in_of_every_provider() {
    let home = Home::new();
    fake_sign_in(&home, "codex", CODEX_SIGNED_IN);
    fake_sign_in(
        &home,
        "opencode",
        &slowly("0.3", "opencode-answered", OPENCODE_SIGNED_OUT),
    );
    let _daemon = home.start();

    let alone = torlek_alone(&home).await;

    let checklist = text(&alone.stdout);
    assert!(alone.status.success(), "{checklist}{}", text(&alone.stderr));
    let sign_in = checklist_step(&checklist, ". Sign in to the provider");
    assert!(sign_in[0].starts_with("[done"), "{checklist}");
    assert!(
        sign_in.iter().any(|line| line
            .trim_start()
            .starts_with("provider opencode sign-in: not signed in")),
        "{checklist}"
    );
}
