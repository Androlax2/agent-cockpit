//! Issue #1279 through the launch path, with Claude Code's own adapter and a fake `claude`
//! program: a clone's harness settings are trusted only when the main checkout's working tree
//! holds them; while that checkout is behind the default branch that does, the refusal names
//! the pull that accepts them, and the reason trust is withheld reaches the owner's dialog.
//! Only the fixture home's `.claude.json` is read and written.

use super::*;

const SETTINGS: &str = "{\"hooks\": {}}\n";
const MCP: &str = "{\"mcpServers\": {\"docs\": {\"command\": \"docs-mcp\"}}}\n";

impl Home {
    /// The home the agent launches with, holding the owner's Claude Code config.
    fn claude_home(&self) -> PathBuf {
        self.path().join("agent-home")
    }

    /// Where the fake `claude` writes the arguments of its latest launch, one per line.
    fn claude_arguments_file(&self) -> PathBuf {
        self.path().join("claude-arguments")
    }

    /// The `--settings` JSON the latest launch of `claude` got.
    fn claude_launch_settings(&self) -> serde_json::Value {
        let file = self.claude_arguments_file();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let arguments = loop {
            if let Ok(arguments) = std::fs::read_to_string(&file) {
                break arguments;
            }
            assert!(std::time::Instant::now() < deadline, "claude never started");
            std::thread::sleep(Duration::from_millis(50));
        };
        let arguments: Vec<&str> = arguments.lines().collect();
        let at = arguments
            .iter()
            .position(|argument| *argument == "--settings")
            .expect("a launch with --settings");
        serde_json::from_str(arguments[at + 1]).unwrap()
    }

    fn claude_config(&self) -> serde_json::Value {
        let text = std::fs::read_to_string(self.claude_home().join(".claude.json")).unwrap();
        serde_json::from_str(&text).unwrap()
    }

    /// Starts a daemon whose one provider is Claude Code's adapter running a program that
    /// sleeps; with `bridge`, its launches count as reporting at the start, which it never
    /// does.
    async fn start_claude(&self, bridge: bool, settings: cockpit_sessions::Settings) -> Daemon {
        let program = self.path().join("claude");
        std::fs::write(
            &program,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nexec sleep 120\n",
                self.claude_arguments_file().display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(
            &program,
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();
        std::fs::create_dir_all(self.claude_home()).unwrap();
        std::fs::write(self.claude_home().join(".claude.json"), "{}\n").unwrap();
        self.start_wired(|| {
            let bridge = bridge.then(|| cockpit_claude::Bridge {
                hook_program: "/unused/cockpit-claude-hook".into(),
                cockpit_program: "/unused/cockpit".into(),
            });
            let claude = cockpit_claude::ClaudeCode::new(program.clone(), None, bridge);
            let mut wiring = common::wiring_in(&self.path(), vec![Arc::new(claude)]);
            wiring.sessions = cockpit_sessions::Settings {
                base_environment: vec![("HOME".into(), self.claude_home().into())],
                ..settings.clone()
            };
            wiring.session_tick = Duration::from_millis(100);
            wiring
        })
        .await
    }
}

/// Commits `files` on a `merged` branch of the project at `root`, makes that commit its
/// fetched default branch (`origin/HEAD`), which its `main` follows, and leaves its working
/// tree on the commit before.
fn merge_behind_the_checkout(root: &Path, files: &[(&str, &str)]) -> String {
    git(root, &["switch", "--quiet", "-c", "merged"]);
    for (path, text) in files {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    git(root, &["add", "."]);
    git(root, &["commit", "--quiet", "-m", "harness settings"]);
    git(root, &["push", "--quiet", "origin", "merged:main"]);
    git(root, &["switch", "--quiet", "main"]);
    git(root, &["fetch", "--quiet", "origin"]);
    git(root, &["remote", "set-head", "origin", "main"]);
    git(
        root,
        &["branch", "--quiet", "--set-upstream-to=origin/main"],
    );
    let output = Process::new("git")
        .current_dir(root)
        .args(["rev-parse", "refs/remotes/origin/HEAD"])
        .output()
        .unwrap();
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

/// The clone's entry in the fixture home's config.
fn clone_entry(home: &Home, session: &SessionView) -> serde_json::Value {
    let clone = PathBuf::from(&session.working_dir).canonicalize().unwrap();
    home.claude_config()["projects"][clone.to_str().unwrap()].clone()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_clone_is_trusted_with_merged_settings_only_once_the_owner_pulls_them() {
    let home = Home::new();
    let daemon = home.start_claude(false, settings()).await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    home.trust_project(&root);
    let merged = merge_behind_the_checkout(
        &root,
        &[(".claude/settings.json", SETTINGS), (".mcp.json", MCP)],
    );

    let behind = start_in(&client, &root, Some("merged")).await;

    assert_eq!(behind.state, SessionStateView::Running);
    assert_eq!(clone_entry(&home, &behind), serde_json::Value::Null);
    let note = behind
        .launch
        .and_then(|launch| launch.harness_trust)
        .unwrap();
    assert!(note.starts_with("not recorded: run `git -C "), "{note}");
    assert!(
        note.ends_with(&format!(
            " pull --ff-only`: the trusted checkout is behind its origin/HEAD at {}, which \
             holds this clone's settings (.claude/settings.json, .mcp.json)",
            &merged[..12]
        )),
        "{note}"
    );

    git(&root, &["merge", "--quiet", "--ff-only", "origin/main"]);
    git(&root, &["branch", "pulled", "merged"]);
    let pulled = start_in(&client, &root, Some("pulled")).await;

    assert_eq!(clone_entry(&home, &pulled)["hasTrustDialogAccepted"], true);
    assert_eq!(
        pulled
            .launch
            .and_then(|launch| launch.harness_trust)
            .as_deref(),
        Some("MCP servers left to Claude Code's prompt: docs")
    );
}

/// Claude Code 2.1.321 saves the owner's approval in the folder's own
/// `.claude/settings.local.json`; a clone's launch gets it in its `--settings`.
#[tokio::test(flavor = "multi_thread")]
async fn the_owners_server_approval_in_the_checkout_reaches_a_clones_launch() {
    let home = Home::new();
    let daemon = home.start_claude(false, settings()).await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    home.trust_project(&root);
    std::fs::write(root.join(".mcp.json"), MCP).unwrap();
    git(&root, &["add", ".mcp.json"]);
    git(&root, &["commit", "--quiet", "-m", "a project server"]);
    std::fs::create_dir_all(root.join(".claude")).unwrap();
    std::fs::write(
        root.join(".claude/settings.local.json"),
        "{\"enabledMcpjsonServers\": [\"docs\"]}",
    )
    .unwrap();

    let session = start_in(&client, &root, Some("approved")).await;

    assert_eq!(
        home.claude_launch_settings()["enabledMcpjsonServers"],
        serde_json::json!(["docs"])
    );
    assert_eq!(session.launch.and_then(|launch| launch.harness_trust), None);
    let clone = PathBuf::from(&session.working_dir);
    assert!(!clone.join(".claude/settings.local.json").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn settings_the_checkout_lacks_withhold_trust_and_the_dialog_says_why() {
    let home = Home::new();
    let daemon = home.start_claude(true, short_report_deadline()).await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    home.trust_project(&root);
    merge_behind_the_checkout(&root, &[(".mcp.json", MCP)]);
    git(&root, &["switch", "--quiet", "-c", "rogue", "merged"]);
    std::fs::write(root.join(".mcp.json"), "{\"mcpServers\": {}}\n").unwrap();
    git(&root, &["commit", "--quiet", "-am", "another server list"]);
    git(&root, &["switch", "--quiet", "main"]);

    let session = start_in(&client, &root, Some("rogue")).await;

    let report = report_once(&client, &session.id, |report| report.needs_input.is_some()).await;
    assert_eq!(report.question_source, Some(QuestionSource::Dialog));
    let dialog = report.needs_input.unwrap();
    assert!(
        dialog.contains(
            "; harness trust: not recorded: its harness settings differ from the trusted \
             checkout's working tree (.mcp.json)"
        ),
        "{dialog}"
    );
    assert_eq!(clone_entry(&home, &session), serde_json::Value::Null);
}
