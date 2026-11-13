//! Issue #1374's acceptance through an in-process daemon's sockets: `sudo` in an approved
//! command asks the owner through Torlek's askpass helper every time, and the password reaches
//! sudo alone, never the command's output, the state directory or a log.
//!
//! Nothing here runs the host's sudo. The trusted sudo is a stand-in the test owns, setuid to
//! the test's own user, passed in code as the daemon's `SudoAskpass`: it asks the helper
//! `SUDO_ASKPASS` names, as `sudo -A` does, then runs its command when the answer matches.
#![cfg(target_os = "linux")]

mod candidate;
mod common;

use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cockpit_agent_channel::{LaunchToken, identity_of};
use cockpit_approvals::{RunSettings, SudoAskpass};
use cockpit_authorization::Permission;
use cockpit_client::{ClientError, Connection};
use cockpit_daemon::{Daemon, Instance, Limits, Options, RUN_LEADER_FLAG, Routes};
use cockpit_protocol::{
    Command, ErrorCode, Reply, RunRequestView, RunResultView, RunStateView, SecretText,
};
use common::{BUILD_PROFILE_DIR, GENEROUS, TestHome, refused};

const PASSWORD: &str = "correct-horse-1374";
const SUDO_PROMPT: &str = "[sudo] password for owner: ";

/// Fields drop in order: the daemon stops before its home is deleted.
struct Asking {
    _daemon: Daemon,
    home: TestHome,
    token: LaunchToken,
    agent: Connection,
    owner: Connection,
    /// The stand-in sudo and the password it expects, outside the daemon's directories.
    fixture: tempfile::TempDir,
    /// Where the asked commands run.
    cwd: tempfile::TempDir,
}

/// A sudo stand-in at `<dir>/sudo`, setuid to its owner as the trusted sudo must be: it takes
/// `-A -k` first, asks `SUDO_ASKPASS` with sudo's prompt, and runs the rest when the answer is
/// the password in `<dir>/expected`.
fn trusted_sudo_stand_in(dir: &Path) -> PathBuf {
    std::fs::write(dir.join("expected"), PASSWORD).unwrap();
    let script = dir.join("sudo");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\n\
             [ \"$1\" = -A ] && [ \"$2\" = -k ] || {{ echo \"stand-in: no -A -k in $*\" >&2; exit 2; }}\n\
             shift 2\n\
             while [ \"$1\" = -A ] || [ \"$1\" = -k ]; do shift; done\n\
             password=$(\"$SUDO_ASKPASS\" '{SUDO_PROMPT}') || exit 1\n\
             [ \"$password\" = \"$(cat '{}')\" ] || {{ echo 'stand-in: wrong password' >&2; exit 1; }}\n\
             exec \"$@\"\n",
            dir.join("expected").display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o4755)).unwrap();
    script
}

async fn started() -> Asking {
    let home = TestHome::new();
    let fixture = tempfile::tempdir().unwrap();
    let sudo = trusted_sudo_stand_in(fixture.path());
    let token = home
        .launch(
            "s1",
            &identity_of(std::process::id()).unwrap(),
            Some(Permission::RunCommands),
        )
        .await;
    let options = Options {
        runtime_dir: home.socket_dir(),
        state_dir: home.state_home().join(BUILD_PROFILE_DIR),
        limits: Limits::default(),
    };
    let helper = candidate::program("torlek-daemon");
    let mut run_settings = RunSettings::new(
        PathBuf::from("/bin/sh"),
        helper.clone(),
        vec![RUN_LEADER_FLAG.into()],
    );
    run_settings.environment = vec![("PATH".into(), "/usr/bin:/bin".into())];
    run_settings.sudo = Some(SudoAskpass::with_stand_in(
        sudo.clone(),
        std::fs::metadata(&sudo).unwrap().uid(),
        options.sudo_launchers(),
        vec![
            helper.into_os_string(),
            cockpit_daemon::SUDO_ASKPASS_FLAG.into(),
            options.askpass_socket_path().into_os_string(),
        ],
    ));
    let mut wiring = common::wiring_in(home.home(), Vec::new());
    wiring.run_settings = run_settings;
    let daemon = Instance::acquire(options)
        .unwrap()
        .start(wiring, |capabilities| {
            Arc::new(Routes::new(
                capabilities,
                cockpit_config::Environment::default(),
            ))
        })
        .await
        .unwrap();
    let agent = Connection::open(daemon.agent_socket_path(), GENEROUS)
        .await
        .unwrap();
    let owner = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    Asking {
        _daemon: daemon,
        home,
        token,
        agent,
        owner,
        fixture,
        cwd: tempfile::tempdir().unwrap(),
    }
}

impl Asking {
    async fn asked(&self, command: &str) -> RunRequestView {
        let cwd = std::fs::canonicalize(self.cwd.path()).unwrap();
        run(self
            .agent
            .request(
                Command::AskRun {
                    launch_token: Some(self.token.as_str().to_owned()),
                    command: command.to_owned(),
                    cwd: cwd.to_str().unwrap().to_owned(),
                    reason: "Proves sudo asks the owner through Torlek.".to_owned(),
                },
                GENEROUS,
            )
            .await)
    }

    /// Approves `request` as soon as the grace after its ask allows.
    async fn approve(&self, request: &RunRequestView) {
        let deadline = Instant::now() + GENEROUS;
        loop {
            let reply = self
                .owner
                .request(
                    Command::ApprovalApprove {
                        request: request.id,
                        revision: request.revision,
                        digest: request.digest.clone(),
                        through: None,
                    },
                    GENEROUS,
                )
                .await;
            let too_soon = matches!(
                &reply,
                Err(ClientError::Daemon(error)) if error.code == ErrorCode::Refused
            );
            if !too_soon {
                run(reply);
                return;
            }
            assert!(Instant::now() < deadline, "never approvable: {reply:?}");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn shown_to_owner(&self, request: i64) -> RunRequestView {
        run(self
            .owner
            .request(Command::ApprovalShow { request }, GENEROUS)
            .await)
    }

    /// The request as the owner reads it once `done` holds for it.
    async fn shown_once(
        &self,
        request: i64,
        done: impl Fn(&RunRequestView) -> bool,
    ) -> RunRequestView {
        let deadline = Instant::now() + GENEROUS;
        loop {
            let shown = self.shown_to_owner(request).await;
            if done(&shown) {
                return shown;
            }
            assert!(Instant::now() < deadline, "never came: {shown:?}");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// The ask the run waits on once the owner sees one numbered `ask`.
    async fn sudo_asked(&self, request: i64, ask: u64) -> RunRequestView {
        self.shown_once(request, |run| {
            run.sudo_ask.as_ref().is_some_and(|asked| asked.ask == ask)
        })
        .await
    }

    async fn answer(&self, request: i64, ask: u64, password: &str) -> Result<Reply, ClientError> {
        self.owner
            .request(
                Command::ApprovalSudoPassword {
                    request,
                    ask,
                    password: SecretText::new(password.to_owned()),
                },
                GENEROUS,
            )
            .await
    }

    async fn ended(&self, request: i64) -> RunRequestView {
        self.shown_once(request, |run| {
            !matches!(
                run.state,
                RunStateView::Asking | RunStateView::Approved | RunStateView::Running
            )
        })
        .await
    }

    /// Runs `command`, answering each of `asks` sudo asks with the password, to its end.
    async fn ran_answering(&self, command: &str, asks: u64) -> RunRequestView {
        let request = self.asked(command).await;
        self.approve(&request).await;
        for ask in 1..=asks {
            self.sudo_asked(request.id, ask).await;
            run(self.answer(request.id, ask, PASSWORD).await);
        }
        self.ended(request.id).await
    }
}

fn run(reply: Result<Reply, ClientError>) -> RunRequestView {
    match reply {
        Ok(Reply::RunRequest { request }) => *request,
        other => panic!("expected a run request, got {other:?}"),
    }
}

/// Every file under `dir` that holds `needle`.
fn files_holding(dir: &Path, needle: &[u8]) -> Vec<PathBuf> {
    let mut holding = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return holding;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            holding.extend(files_holding(&path, needle));
        } else if kind.is_file()
            && std::fs::read(&path)
                .is_ok_and(|bytes| bytes.windows(needle.len()).any(|window| window == needle))
        {
            holding.push(path);
        }
    }
    holding
}

#[tokio::test(flavor = "multi_thread")]
async fn the_owner_reads_the_sudo_flags_the_run_uses() {
    let asking = started().await;

    let request = asking.asked("sudo true && sudo -A -k true").await;

    assert_eq!(request.command, "sudo -A -k true && sudo -A -k true");
}

#[tokio::test(flavor = "multi_thread")]
async fn each_sudo_in_a_run_asks_the_owner_again() {
    let asking = started().await;

    let ended = asking
        .ran_answering("sudo echo first-ran && sudo echo second-ran", 2)
        .await;

    let result = ended.result.expect("a result");
    assert_eq!(ended.state, RunStateView::Finished, "{}", result.output);
    assert_eq!(result.exit_code, 0, "{}", result.output);
    assert!(result.output.contains("first-ran"), "{}", result.output);
    assert!(result.output.contains("second-ran"), "{}", result.output);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_owner_sees_sudos_prompt_while_it_asks() {
    let asking = started().await;
    let request = asking.asked("sudo true").await;
    asking.approve(&request).await;

    let shown = asking.sudo_asked(request.id, 1).await;

    assert!(shown.waiting_for_input, "{shown:?}");
    let asked = shown.sudo_ask.expect("an ask");
    assert_eq!(asked.prompt.text, SUDO_PROMPT);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_password_reaches_no_output_state_or_runtime_file() {
    let asking = started().await;

    let ended = asking.ran_answering("sudo echo as-root", 1).await;

    let output = ended.result.expect("a result").output;
    assert!(output.contains("as-root"), "{output}");
    assert!(!output.contains(PASSWORD), "{output}");
    let state = asking.home.state_home();
    let runtime = asking.home.socket_dir();
    assert_eq!(
        files_holding(&state, PASSWORD.as_bytes()),
        Vec::<PathBuf>::new()
    );
    assert_eq!(
        files_holding(&runtime, PASSWORD.as_bytes()),
        Vec::<PathBuf>::new()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_helper_run_by_the_command_itself_is_refused_and_asks_nothing() {
    let asking = started().await;
    let request = asking
        .asked("\"$SUDO_ASKPASS\" 'Password: '; echo \"helper exited $?\"")
        .await;
    asking.approve(&request).await;

    let ended = asking.ended(request.id).await;

    let output = ended.result.expect("a result").output;
    assert!(output.contains("nothing was asked"), "{output}");
    assert!(output.contains("helper exited 1"), "{output}");
    assert_eq!(ended.sudo_ask, None);
}

/// The output of a run whose `sudo` runs the askpass at `askpass`, once it ended unasked.
async fn ran_with_own_askpass(asking: &Asking, askpass: &str) -> String {
    let command = format!("SUDO_ASKPASS='{askpass}' sudo echo as-root; echo \"sudo exited $?\"");
    let request = asking.asked(&command).await;
    asking.approve(&request).await;
    let ended = asking.ended(request.id).await;
    assert_eq!(ended.sudo_ask, None);
    ended.result.expect("a result").output
}

/// An executable `#!/bin/sh` script at `<dir>/<name>` running `body`.
fn script(dir: &Path, name: &str, body: &str) -> String {
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    path.to_str().unwrap().to_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn an_askpass_of_the_commands_own_under_sudo_is_refused() {
    let asking = started().await;
    let copy = asking.fixture.path().join("copied-helper");
    std::fs::copy(candidate::program("torlek-daemon"), &copy).unwrap();
    let socket = asking.home.socket_dir().join("askpass.sock");
    let askpass = script(
        asking.fixture.path(),
        "own-askpass",
        &format!(
            "exec '{}' --sudo-askpass '{}' \"$@\"",
            copy.display(),
            socket.display()
        ),
    );

    let output = ran_with_own_askpass(&asking, &askpass).await;

    assert!(
        output.contains("is not Torlek's askpass helper"),
        "{output}"
    );
    assert!(output.contains("sudo exited 1"), "{output}");
    assert!(!output.contains("as-root"), "{output}");
}

#[tokio::test(flavor = "multi_thread")]
async fn torleks_helper_with_its_output_sent_elsewhere_is_refused() {
    let asking = started().await;
    let stolen = asking.fixture.path().join("stolen");
    let socket = asking.home.socket_dir().join("askpass.sock");
    let askpass = script(
        asking.fixture.path(),
        "redirecting-askpass",
        &format!(
            "exec '{}' --sudo-askpass '{}' \"$@\" > '{}'",
            candidate::program("torlek-daemon").display(),
            socket.display(),
            stolen.display()
        ),
    );

    let output = ran_with_own_askpass(&asking, &askpass).await;

    assert!(output.contains("not a pipe to sudo"), "{output}");
    assert!(output.contains("sudo exited 1"), "{output}");
    assert!(
        !std::fs::read_to_string(&stolen)
            .unwrap_or_default()
            .contains(PASSWORD)
    );
}

/// A sudo stand-in at `<dir>/traced/sudo` that traces the helper it starts, as an approved
/// command may trace its own descendants: a Python script, so `/proc` names it `sudo` as it
/// names the trusted one. It starts `SUDO_ASKPASS` under `PTRACE_TRACEME`, lets it run to its
/// end, and prints what it read from it.
fn tracing_sudo_stand_in(dir: &Path) -> PathBuf {
    let traced = dir.join("traced");
    std::fs::create_dir(&traced).unwrap();
    let script = traced.join("sudo");
    std::fs::write(
        &script,
        "#!/usr/bin/python3\n\
         import ctypes, os, signal\n\
         libc = ctypes.CDLL(None, use_errno=True)\n\
         libc.ptrace.argtypes = [ctypes.c_long, ctypes.c_long, ctypes.c_void_p, ctypes.c_void_p]\n\
         PTRACE_TRACEME, PTRACE_CONT = 0, 7\n\
         askpass = os.environ['SUDO_ASKPASS']\n\
         read, write = os.pipe()\n\
         helper = os.fork()\n\
         if helper == 0:\n\
         \x20   os.dup2(write, 1)\n\
         \x20   libc.ptrace(PTRACE_TRACEME, 0, None, None)\n\
         \x20   os.execv(askpass, [askpass, 'Password: '])\n\
         os.close(write)\n\
         while True:\n\
         \x20   _, status = os.waitpid(helper, 0)\n\
         \x20   if not os.WIFSTOPPED(status):\n\
         \x20       break\n\
         \x20   stopped = os.WSTOPSIG(status)\n\
         \x20   libc.ptrace(PTRACE_CONT, helper, None, 0 if stopped == signal.SIGTRAP else stopped)\n\
         print('tracer read %d bytes' % len(os.read(read, 4096)))\n",
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    script
}

#[tokio::test(flavor = "multi_thread")]
async fn a_traced_helper_refuses_and_asks_nothing() {
    let asking = started().await;
    let tracing_sudo = tracing_sudo_stand_in(asking.fixture.path());
    let request = asking
        .asked(&format!(
            "'{}' -A -k true; echo \"sudo exited $?\"",
            tracing_sudo.display()
        ))
        .await;
    asking.approve(&request).await;

    let ended = asking.ended(request.id).await;

    let output = ended.result.expect("a result").output;
    assert!(
        output.contains("traces it, so nothing was asked"),
        "{output}"
    );
    assert!(output.contains("tracer read 0 bytes"), "{output}");
    assert_eq!(ended.sudo_ask, None);
}

/// The pid of the askpass helper asking on `socket`, found by its command line.
fn helper_asking_on(socket: &Path) -> u32 {
    let wanted = [
        cockpit_daemon::SUDO_ASKPASS_FLAG.as_bytes(),
        socket.as_os_str().as_encoded_bytes(),
    ];
    std::fs::read_dir("/proc")
        .unwrap()
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
        .find(|pid| {
            std::fs::read(format!("/proc/{pid}/cmdline")).is_ok_and(|cmdline| {
                let words: Vec<&[u8]> = cmdline.split(|byte| *byte == 0).collect();
                words.windows(2).any(|pair| pair == wanted)
            })
        })
        .expect("a helper asking")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_waiting_helpers_descriptors_and_memory_are_closed_to_its_own_user() {
    let asking = started().await;
    let request = asking.asked("sudo true").await;
    asking.approve(&request).await;
    asking.sudo_asked(request.id, 1).await;
    let helper = helper_asking_on(&asking.home.socket_dir().join("askpass.sock"));

    let descriptors = std::fs::read_dir(format!("/proc/{helper}/fd")).map(|_| ());
    let memory = std::fs::File::open(format!("/proc/{helper}/mem")).map(|_| ());

    assert_eq!(
        descriptors.map_err(|error| error.kind()),
        Err(std::io::ErrorKind::PermissionDenied)
    );
    assert_eq!(
        memory.map_err(|error| error.kind()),
        Err(std::io::ErrorKind::PermissionDenied)
    );
    run(asking.answer(request.id, 1, PASSWORD).await);
}

/// Another sudo stand-in: `<dir>/<kind>/sudo`, a link to bash, so `/proc` names it as the
/// trusted sudo and reads its arguments as a program's, not a script's. The asked command
/// gives it `-c` and what it runs; bash takes `-k` too, harmlessly, as sudo would.
fn other_sudo_stand_in(dir: &Path, kind: &str) -> PathBuf {
    let other = dir.join(kind);
    std::fs::create_dir(&other).unwrap();
    let sudo = other.join("sudo");
    std::os::unix::fs::symlink("/bin/bash", &sudo).unwrap();
    sudo
}

/// The result of an approved `command` that asks the owner nothing.
async fn ran_unasked(asking: &Asking, command: &str) -> RunResultView {
    let request = asking.asked(command).await;
    asking.approve(&request).await;
    asking.ended(request.id).await.result.expect("a result")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sudo_by_its_path_that_asked_nothing_is_reported() {
    let asking = started().await;
    // As sudo does when an earlier sudo in the run left an unlock: it runs, asking nothing,
    // after loading its policy.
    let reusing = other_sudo_stand_in(asking.fixture.path(), "reusing");

    let result = ran_unasked(
        &asking,
        &format!("'{}' -c 'sleep 0.3; echo as-root'", reusing.display()),
    )
    .await;

    assert!(result.output.contains("as-root"), "{}", result.output);
    assert_eq!(result.sudo_ran_unasked, 1, "{}", result.output);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sudo_given_k_that_asked_nothing_is_reported_too() {
    let asking = started().await;
    // `-k` does not prove the owner was asked: the command may hand sudo a password itself.
    let supplied = other_sudo_stand_in(asking.fixture.path(), "supplied");

    let result = ran_unasked(
        &asking,
        &format!("'{}' -k -c 'sleep 0.3; echo as-root'", supplied.display()),
    )
    .await;

    assert!(result.output.contains("as-root"), "{}", result.output);
    assert_eq!(result.sudo_ran_unasked, 1, "{}", result.output);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sudo_that_asked_the_owner_is_not_reported() {
    let asking = started().await;
    let asking_sudo = other_sudo_stand_in(asking.fixture.path(), "asking");
    let request = asking
        .asked(&format!(
            "'{}' -c 'password=$(\"$SUDO_ASKPASS\" \"Password: \") || exit 1; echo as-root'",
            asking_sudo.display()
        ))
        .await;
    asking.approve(&request).await;
    asking.sudo_asked(request.id, 1).await;
    run(asking.answer(request.id, 1, PASSWORD).await);

    let ended = asking.ended(request.id).await;

    let result = ended.result.expect("a result");
    assert_eq!(result.exit_code, 0, "{}", result.output);
    assert_eq!(result.sudo_ran_unasked, 0, "{}", result.output);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_running_command_shows_its_unasked_sudo_once_that_sudo_ended() {
    let asking = started().await;
    let reusing = other_sudo_stand_in(asking.fixture.path(), "reusing");
    let request = asking
        .asked(&format!(
            "'{}' -c 'sleep 0.3; true'; sleep 30",
            reusing.display()
        ))
        .await;
    asking.approve(&request).await;

    let shown = asking
        .shown_once(request.id, |run| run.sudo_ran_unasked == 1)
        .await;

    assert_eq!(shown.state, RunStateView::Running);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_helper_outside_any_run_is_refused_and_prints_no_password() {
    let asking = started().await;
    let socket = asking.home.socket_dir().join("askpass.sock");

    // The helper starts no tmux; its environment is the test home's all the same, as for
    // every run of the daemon program here.
    let helper = tokio::process::Command::new(candidate::program("torlek-daemon"))
        .env_clear()
        .env("HOME", asking.home.home())
        .env("TMUX_TMPDIR", &asking.home.tmux_tmpdir)
        .arg(cockpit_daemon::SUDO_ASKPASS_FLAG)
        .arg(&socket)
        .arg("Password: ")
        .stdin(Stdio::null())
        .output()
        .await
        .unwrap();

    assert!(!helper.status.success());
    assert!(helper.stdout.is_empty(), "{:?}", helper.stdout);
    let refusal = String::from_utf8_lossy(&helper.stderr);
    assert!(refusal.contains("nothing was asked"), "{refusal}");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_answer_to_no_open_ask_is_refused() {
    let asking = started().await;
    let request = asking.asked("sudo true").await;
    asking.approve(&request).await;
    asking.sudo_asked(request.id, 1).await;

    let refusal = refused(asking.answer(request.id, 2, PASSWORD).await);

    assert_eq!(refusal.code, ErrorCode::Conflict, "{refusal:?}");
    run(asking.answer(request.id, 1, PASSWORD).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sudo_left_unanswered_ends_with_its_command_and_asks_no_more() {
    let asking = started().await;
    let request = asking.asked("sudo true").await;
    asking.approve(&request).await;
    asking.sudo_asked(request.id, 1).await;

    run(asking
        .owner
        .request(
            Command::ApprovalStop {
                request: request.id,
            },
            GENEROUS,
        )
        .await);
    let ended = asking.ended(request.id).await;

    assert_eq!(ended.sudo_ask, None);
    let refusal = refused(asking.answer(request.id, 1, PASSWORD).await);
    assert_ne!(refusal.code, ErrorCode::Internal, "{refusal:?}");
}

/// Set, in the environment of this test binary run again, to run the release path's check.
const OVERRIDDEN_ENVIRONMENT: &str = "TORLEK_TEST_SUDO_OVERRIDES_SET";
const RELEASE_PATH_TEST: &str =
    "the_daemons_own_askpass_answers_the_hosts_sudo_whatever_its_environment";

/// What the release path does with each variable that could name another sudo set in the
/// daemon's own environment: the test binary runs itself again with them set, since this
/// workspace denies the unsafe `set_var`.
#[test]
fn the_daemons_own_askpass_answers_the_hosts_sudo_whatever_its_environment() {
    if std::env::var_os(OVERRIDDEN_ENVIRONMENT).is_none() {
        let fixture = tempfile::tempdir().unwrap();
        let other = fixture.path().join("sudo");
        std::os::unix::fs::symlink("/bin/true", &other).unwrap();
        let again = std::process::Command::new(std::env::current_exe().unwrap())
            .args([RELEASE_PATH_TEST, "--exact", "--nocapture"])
            .env(OVERRIDDEN_ENVIRONMENT, "1")
            .env("TORLEK_SUDO", &other)
            .env("SUDO", &other)
            .env("SUDO_ASKPASS", fixture.path().join("askpass"))
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", other.parent().unwrap().display()),
            )
            .output()
            .unwrap();
        let printed = String::from_utf8_lossy(&again.stdout);
        assert!(
            again.status.success(),
            "{printed}{}",
            String::from_utf8_lossy(&again.stderr)
        );
        assert!(printed.contains("1 passed"), "{printed}");
        return;
    }
    let launchers = tempfile::tempdir().unwrap();

    let askpass = cockpit_daemon::sudo_askpass(
        Path::new("/proc/self/exe"),
        Path::new("/nonexistent/askpass.sock"),
        launchers.path().to_path_buf(),
    );

    // Only read, never run: whether the host's sudo is a root-owned setuid file.
    let host_sudo_is_trusted = std::fs::metadata(cockpit_approvals::TRUSTED_SUDO)
        .is_ok_and(|sudo| sudo.uid() == 0 && sudo.mode() & 0o4000 != 0 && sudo.mode() & 0o022 == 0);
    match askpass {
        Some(askpass) => {
            assert_eq!(askpass.sudo(), Path::new("/usr/bin/sudo"));
            assert_eq!(askpass.sudo_owner(), 0);
        }
        None => assert!(
            !host_sudo_is_trusted,
            "the host's sudo holds, yet askpass is off"
        ),
    }
}

#[test]
fn a_runs_environment_names_no_sudo() {
    let overrides = vec![
        ("SUDO_ASKPASS".into(), "/elsewhere/askpass".into()),
        ("TORLEK_SUDO".into(), "/elsewhere/sudo".into()),
    ];

    let settings = cockpit_daemon::run_settings(overrides).unwrap();

    assert!(settings.sudo.is_none(), "{:?}", settings.sudo);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sudo_within_a_sudo_is_counted_once() {
    let asking = started().await;
    let outer = other_sudo_stand_in(asking.fixture.path(), "outer");
    let inner = other_sudo_stand_in(asking.fixture.path(), "inner");

    let result = ran_unasked(
        &asking,
        &format!(
            "'{}' -c \"'{}' -c 'sleep 0.3; echo as-root'; sleep 0.3\"",
            outer.display(),
            inner.display()
        ),
    )
    .await;

    assert!(result.output.contains("as-root"), "{}", result.output);
    assert_eq!(result.sudo_ran_unasked, 1, "{}", result.output);
}
