//! Every launch goes through the sandbox (#61), through the real daemon binary with a
//! stand-in Greywall that logs its arguments and runs the agent: each launch logs in to
//! greyproxy under a client identity of its own, a host without Greywall refuses launches
//! naming why, only the owner's `agents.sandbox = "off"` launches unsandboxed, and
//! `cockpit session sandbox` and `cockpit sandbox show` print the evidence and the profile.
//! An agent writes its session's directory (#437), and a shared desktop session mounts the
//! display endpoints its variables name (#435).

mod real_daemon;

use std::collections::HashSet;
use std::time::Duration;

use cockpit_client::{ClientError, Connection};
use cockpit_protocol::{Command, ErrorCode, Reply};
use real_daemon::{GENEROUS, Home, SESSION_DIR_PROJECT, new_session, session_new_command};

fn sandbox_launches(home: &Home) -> Vec<String> {
    std::fs::read_to_string(home.sandbox_log())
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// The value after `flag` in one logged launch.
fn argument_after<'line>(launch: &'line str, flag: &str) -> &'line str {
    let words: Vec<&str> = launch.split(' ').collect();
    let at = words
        .iter()
        .position(|word| *word == flag)
        .unwrap_or_else(|| panic!("{flag} missing from {launch}"));
    words[at + 1]
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_launch_runs_the_agent_inside_greywall_as_its_own_proxy_client() {
    let home = Home::new();
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, "app").await;

    let session = new_session(&client, &root).await;
    home.wait_for_launches(&session.id, 1).await;

    let launches = sandbox_launches(&home);
    assert_eq!(launches.len(), 1, "{launches:?}");
    let launch = &launches[0];
    assert!(
        argument_after(launch, "--proxy-user").starts_with(&format!("cockpit-{}-", session.id)),
        "{launch}"
    );
    assert_eq!(argument_after(launch, "--profile"), "claude");
    assert_eq!(
        argument_after(launch, "--allow-path"),
        root.to_str().unwrap()
    );
    assert_eq!(
        argument_after(launch, "--ignore-secret"),
        "COCKPIT_LAUNCH_TOKEN"
    );
    assert!(
        argument_after(launch, "--").ends_with("/bin/claude"),
        "{launch}"
    );
}

/// The folders the first launch of a new session may read (`--allow-read-path`).
async fn readable_folders(home: &Home) -> Vec<String> {
    let client = home.connect().await;
    let (root, _) = home.project(&client, "app").await;
    let session = new_session(&client, &root).await;
    home.wait_for_launches(&session.id, 1).await;
    let words: Vec<String> = sandbox_launches(home)[0]
        .split(' ')
        .map(str::to_owned)
        .collect();
    words
        .windows(2)
        .filter(|pair| pair[0] == "--allow-read-path")
        .map(|pair| pair[1].clone())
        .collect()
}

fn state_folder(home: &Home, folder: &str) -> String {
    home.state_dir().join(folder).display().to_string()
}

/// Greywall denies every read it was not told to allow: the instruction bundle the launch
/// brief points at (#419) and the OpenCode plugin (#415) must be readable.
#[tokio::test(flavor = "multi_thread")]
async fn a_launch_may_read_the_instruction_bundle_and_the_opencode_plugin() {
    let home = Home::new();
    let _daemon = home.start();

    let readable = readable_folders(&home).await;

    for folder in ["agent-context", "opencode-plugin"] {
        assert!(
            readable.contains(&state_folder(&home, folder)),
            "{folder} is not readable: {readable:?}"
        );
    }
}

/// The Linux Landlock wrapper re-executes the owned Greywall inside the sandbox.
/// Its directory must remain readable, while the agent cannot replace engine files.
#[tokio::test(flavor = "multi_thread")]
async fn managed_engines_are_read_only_and_never_hidden_from_the_wrapper() {
    let home = Home::new();
    let _daemon = home.start();
    let engines = state_folder(&home, "engines");

    let readable = readable_folders(&home).await;

    assert!(readable.contains(&engines), "{readable:?}");
    let launch = &sandbox_launches(&home)[0];
    let words: Vec<_> = launch.split(' ').collect();
    for pair in words.windows(2).filter(|pair| pair[0] == "--allow-path") {
        assert!(!engines.starts_with(pair[1]), "writable engines: {launch}");
    }
    let settings: serde_json::Value =
        serde_json::from_slice(&std::fs::read(argument_after(launch, "--settings")).unwrap())
            .unwrap();
    assert!(
        settings["filesystem"]["denyWrite"]
            .as_array()
            .unwrap()
            .contains(&engines.clone().into()),
        "engines need an explicit write denial: {settings}"
    );
    for denied in settings["filesystem"]["denyRead"].as_array().unwrap() {
        assert!(
            !engines.starts_with(denied.as_str().unwrap()),
            "hidden engines: {settings}"
        );
    }
}

/// #419: Claude Code lists the bundle's `cockpit` skill because the launch adds the folder
/// whose `.claude/skills` holds it, written at the daemon's start and readable in the sandbox.
#[tokio::test(flavor = "multi_thread")]
async fn a_claude_launch_adds_the_instruction_skills_it_may_read() {
    let home = Home::new();
    let _daemon = home.start();
    let skills_root = state_folder(&home, "claude-skills");

    let readable = readable_folders(&home).await;

    assert!(readable.contains(&skills_root), "{readable:?}");
    assert_eq!(
        argument_after(&sandbox_launches(&home)[0], "--add-dir"),
        skills_root
    );
    let skill = std::fs::read_to_string(
        std::path::Path::new(&skills_root).join(".claude/skills/cockpit/SKILL.md"),
    )
    .unwrap();
    assert!(skill.starts_with("---\nname: cockpit\n"), "{skill}");
}

/// `--add-dir` lets Claude Code's own tools edit the folder, so only the sandbox keeps one
/// session from rewriting the skill every later session loads: it is never writable.
#[tokio::test(flavor = "multi_thread")]
async fn no_launch_may_write_the_instruction_skills() {
    let home = Home::new();
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, "app").await;
    let session = new_session(&client, &root).await;
    home.wait_for_launches(&session.id, 1).await;
    let skills_root = state_folder(&home, "claude-skills");
    let launch = &sandbox_launches(&home)[0];

    let writable: Vec<&str> = launch
        .split(' ')
        .collect::<Vec<_>>()
        .windows(2)
        .filter(|pair| pair[0] == "--allow-path")
        .map(|pair| pair[1])
        .collect();

    assert!(
        writable
            .iter()
            .all(|path| !skills_root.starts_with(path) && !path.starts_with(&skills_root)),
        "{writable:?}"
    );
}

/// Without OpenCode the plugin folder is never written; a path that does not exist handed
/// to the sandbox would fail every other agent's launch.
#[tokio::test(flavor = "multi_thread")]
async fn without_opencode_a_launch_is_given_no_plugin_folder() {
    let home = Home::new();
    std::fs::remove_file(home.path().join("bin/opencode")).unwrap();
    // A folder holding `opencode` (often `/usr/bin`, beside Git and tmux) is replaced by a
    // copy of its links without it.
    let mirrors = tempfile::tempdir().unwrap();
    let path = std::env::var_os("PATH").unwrap_or_default();
    let search = std::iter::once(home.path().join("bin")).chain(
        std::env::split_paths(&path)
            .enumerate()
            .map(|(index, dir)| {
                if !dir.join("opencode").exists() {
                    return dir;
                }
                let mirror = mirrors.path().join(index.to_string());
                std::fs::create_dir(&mirror).unwrap();
                for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                    if entry.file_name() != "opencode" {
                        std::os::unix::fs::symlink(entry.path(), mirror.join(entry.file_name()))
                            .unwrap();
                    }
                }
                mirror
            }),
    );
    let search = std::env::join_paths(search).unwrap();
    let _daemon = home.start_with(&[("PATH", search.to_str().unwrap())]);

    let readable = readable_folders(&home).await;

    assert!(
        readable.contains(&state_folder(&home, "agent-context")),
        "{readable:?}"
    );
    assert!(
        !readable.contains(&state_folder(&home, "opencode-plugin")),
        "{readable:?}"
    );
}

/// Git and the project's own files live at the checkout's root: a session started in a
/// subfolder still gets the whole checkout.
#[tokio::test(flavor = "multi_thread")]
async fn a_session_started_in_a_subfolder_gets_its_whole_checkout() {
    let home = Home::new();
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, "app").await;
    let subfolder = root.join("crates/core");
    std::fs::create_dir_all(&subfolder).unwrap();

    let session = new_session(&client, &subfolder).await;
    home.wait_for_launches(&session.id, 1).await;

    let launch = &sandbox_launches(&home)[0];
    assert_eq!(
        argument_after(launch, "--allow-path"),
        root.to_str().unwrap()
    );
}

/// Greyproxy keeps one client secret per name; four launches under one name would leave the
/// earlier sandboxes without network (stack proposal 00f3a41c). Restarts are the launches
/// most likely to overlap: the same session, back to back.
#[tokio::test(flavor = "multi_thread")]
async fn four_back_to_back_launches_of_a_session_log_in_as_four_proxy_clients() {
    let home = Home::new();
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, "app").await;
    let session = new_session(&client, &root).await;
    home.wait_for_launches(&session.id, 1).await;

    for restart in 2..=4 {
        let restarted = client
            .request(
                Command::SessionRestart {
                    session: session.id.clone(),
                },
                Duration::from_secs(60),
            )
            .await;
        assert!(
            matches!(restarted, Ok(Reply::Session { .. })),
            "{restarted:?}"
        );
        home.wait_for_launches(&session.id, restart).await;
    }

    let clients: HashSet<String> = sandbox_launches(&home)
        .iter()
        .map(|launch| argument_after(launch, "--proxy-user").to_owned())
        .collect();
    assert_eq!(clients.len(), 4, "{clients:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn without_greywall_a_launch_is_refused_naming_why() {
    let home = Home::new();
    let _daemon = home.start_without_greywall();
    let client = home.connect().await;
    let (root, _) = home.project(&client, "app").await;

    let refused = client
        .request(session_new_command(&root), Duration::from_secs(60))
        .await;

    assert!(
        matches!(&refused, Err(ClientError::Daemon(error))
            if error.code == ErrorCode::Refused
                && error.message.contains("/engines/") && error.message.contains("greywall does not run")),
        "{refused:?}"
    );
    assert!(home.launches().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_owners_off_setting_launches_unsandboxed_and_the_evidence_says_so() {
    let home = Home::new();
    home.write_owner_config("[agents]\nsandbox = \"off\"\n");
    let _daemon = home.start_without_greywall();
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
            && first.ends_with("unsandboxed: the owner set agents.sandbox = \"off\""),
        "{first}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn cockpit_session_sandbox_prints_how_the_latest_launch_ran() {
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
    let printed = text(&shown.stdout);
    let proxy_user = argument_after(&sandbox_launches(&home)[0], "--proxy-user").to_owned();
    for expected in [
        "sandbox: enforced by greywall on ".to_owned(),
        format!("proxy client: {proxy_user}\n"),
        format!("rule: read-write {} <- greywall ", root.display()),
        "rule: read everything else: denied <- greywall ".to_owned(),
        "rule: network: through greyproxy only <- greywall ".to_owned(),
        "observed: network isolation: yes\n".to_owned(),
    ] {
        assert!(
            printed.contains(&expected),
            "{expected:?} missing from:\n{printed}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn cockpit_sandbox_show_prints_the_profile_without_launching() {
    let home = Home::new();
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, "app").await;

    let shown = home
        .cockpit(
            &[
                "sandbox",
                "show",
                root.to_str().unwrap(),
                "--agent",
                "claude",
            ],
            &[],
        )
        .await;

    assert!(shown.status.success(), "{}", text(&shown.stderr));
    let printed = text(&shown.stdout);
    assert!(
        printed.contains(&format!("rule: read-write {} <- ", root.display())),
        "{printed}"
    );
    let command = printed
        .lines()
        .find_map(|line| line.strip_prefix("command: "))
        .unwrap_or_else(|| panic!("no command line in:\n{printed}"));
    let engine = cockpit_sandbox::install::programs(&home.state_dir()).unwrap();
    assert!(
        command.contains(&format!("{} --settings ", engine.greywall.display())),
        "{command}"
    );
    assert!(command.ends_with(" -- <agent>"), "{command}");
    assert!(sandbox_launches(&home).is_empty());
    assert!(home.launches().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn cockpit_sandbox_show_without_greywall_fails_naming_why() {
    let home = Home::new();
    let _daemon = home.start_without_greywall();
    let client = home.connect().await;
    let (root, _) = home.project(&client, "app").await;

    let shown = home
        .cockpit(
            &[
                "sandbox",
                "show",
                root.to_str().unwrap(),
                "--agent",
                "claude",
            ],
            &[],
        )
        .await;

    assert!(!shown.status.success());
    assert!(
        text(&shown.stderr).contains("/engines/")
            && text(&shown.stderr).contains("greywall does not run"),
        "{}",
        text(&shown.stderr)
    );
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

/// A provider keeps what reaches the running agent in the session's directory (Codex's
/// app-server socket): the agent, inside the sandbox, writes there (#437).
#[tokio::test(flavor = "multi_thread")]
async fn a_sandboxed_agent_writes_its_sessions_directory() {
    let home = Home::new();
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, SESSION_DIR_PROJECT).await;

    let session = new_session(&client, &root).await;
    home.wait_for_launches(&session.id, 1).await;

    let probe = home.session_dir(&session.id).join("probe");
    let deadline = std::time::Instant::now() + GENEROUS;
    while !probe.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "the agent wrote no {}; greywall ran with {:?}",
            probe.display(),
            sandbox_launches(&home)
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let shown = home
        .cockpit(&["session", "sandbox", &session.id], &[])
        .await;
    assert!(
        text(&shown.stdout).contains(&format!(
            "rule: read-write {} (the session's directory) <- greywall ",
            home.session_dir(&session.id).display()
        )),
        "{}",
        text(&shown.stdout)
    );
}

/// The desktop variables alone reach nothing under `defaultDenyRead`: the launch mounts each
/// socket and file they name, and the evidence says so (#435).
#[tokio::test(flavor = "multi_thread")]
async fn a_shared_desktop_launch_mounts_the_display_endpoints_its_variables_name() {
    let home = Home::new();
    let bus = home.owner_bus();
    let _daemon = home.start_with(&[
        ("DISPLAY", ":7"),
        ("WAYLAND_DISPLAY", "wayland-7"),
        ("XAUTHORITY", "/run/user/7/gdm/Xauthority"),
        ("DBUS_SESSION_BUS_ADDRESS", &bus),
    ]);
    let client = home.connect().await;
    let (root, _) = home.project(&client, "app").await;

    let session = new_session(&client, &root).await;
    home.wait_for_launches(&session.id, 1).await;

    let launch = &sandbox_launches(&home)[0];
    let wayland = home.path().join("run/wayland-7");
    for mounted in [
        "--allow-path /tmp/.X11-unix/X7 ".to_owned(),
        format!("--allow-path {} ", wayland.display()),
        "--allow-read-path /run/user/7/gdm/Xauthority ".to_owned(),
    ] {
        assert!(
            launch.contains(&mounted),
            "{mounted:?} missing from {launch}"
        );
    }
    let shown = home
        .cockpit(&["session", "sandbox", &session.id], &[])
        .await;
    let printed = text(&shown.stdout);
    let bus_path = bus.trim_start_matches("unix:path=");
    for expected in [
        "rule: desktop socket /tmp/.X11-unix/X7 for DISPLAY <- greywall ".to_owned(),
        format!(
            "rule: desktop socket {} for WAYLAND_DISPLAY <- greywall ",
            wayland.display()
        ),
        "rule: desktop file /run/user/7/gdm/Xauthority for XAUTHORITY <- greywall ".to_owned(),
        format!(
            "rule: desktop bus {bus_path} for DBUS_SESSION_BUS_ADDRESS <- greywall's xdg-dbus-proxy"
        ),
    ] {
        assert!(
            printed.contains(&expected),
            "{expected:?} missing from:\n{printed}"
        );
    }
}
