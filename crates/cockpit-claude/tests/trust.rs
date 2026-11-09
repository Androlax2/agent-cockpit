//! Issue #702: Claude Code's folder trust recorded for a parallel clone, against fixture
//! config files only; the owner's own `~/.claude.json` is never read.

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use cockpit_claude::ClaudeCode;
use cockpit_claude::trust::{self, Recorded};
use cockpit_provider_api::{DefaultBranch, Provider, TrustRequest, WorkspaceTrust};

/// A config as Claude Code 2.1.317 writes one: two-space indents, other projects, other keys.
const CONFIG: &str = r#"{
  "numStartups": 12,
  "projects": {
    "/home/owner/other": {
      "allowedTools": [],
      "hasTrustDialogAccepted": true,
      "lastCost": 0.5
    }
  },
  "userID": "abc\"def"
}
"#;

struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new(config: &str) -> Fixture {
        let fixture = Fixture {
            dir: tempfile::tempdir().unwrap(),
        };
        fs::create_dir(fixture.workspace()).unwrap();
        fs::write(fixture.config(), config).unwrap();
        fs::set_permissions(fixture.config(), fs::Permissions::from_mode(0o600)).unwrap();
        fixture
    }

    fn config(&self) -> PathBuf {
        self.dir.path().join(".claude.json")
    }

    fn workspace(&self) -> PathBuf {
        self.dir.path().join("clone")
    }

    fn key(&self) -> String {
        self.workspace()
            .canonicalize()
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned()
    }

    fn read(&self) -> String {
        fs::read_to_string(self.config()).unwrap()
    }

    fn trust_of(&self, key: &str) -> Option<serde_json::Value> {
        let config: serde_json::Value = serde_json::from_str(&self.read()).unwrap();
        config["projects"][key]
            .get("hasTrustDialogAccepted")
            .cloned()
    }
}

#[test]
fn a_new_clone_gets_claude_codes_own_entry_and_every_other_byte_stays() {
    let fixture = Fixture::new(CONFIG);

    let recorded = trust::record(
        &fixture.config(),
        &fixture.workspace(),
        &trust::Change::Trust(trust::McpApprovals::default()),
        trust::WhenMissing::Refuse,
    )
    .unwrap();

    assert_eq!(recorded, Recorded::Written);
    let written = fixture.read();
    let entry = format!(
        "{}:{{\"allowedTools\":[],\"mcpContextUris\":[],\"mcpServers\":{{}},\
         \"enabledMcpjsonServers\":[],\"disabledMcpjsonServers\":[],\
         \"hasTrustDialogAccepted\":true,\"hasClaudeMdExternalIncludesApproved\":false,\
         \"hasClaudeMdExternalIncludesWarningShown\":false}},",
        serde_json::to_string(&fixture.key()).unwrap()
    );
    assert_eq!(written.replacen(&entry, "", 1), CONFIG);
}

#[test]
fn an_entry_that_refused_trust_has_only_its_value_turned_true() {
    let config = CONFIG.replace("/home/owner/other", "WORKSPACE").replace(
        "\"hasTrustDialogAccepted\": true",
        "\"hasTrustDialogAccepted\": false",
    );
    let fixture = Fixture::new("");
    let config = config.replace("WORKSPACE", &fixture.key());
    fs::write(fixture.config(), &config).unwrap();

    trust::record(
        &fixture.config(),
        &fixture.workspace(),
        &trust::Change::Trust(trust::McpApprovals::default()),
        trust::WhenMissing::Refuse,
    )
    .unwrap();

    assert_eq!(
        fixture.read(),
        config.replace(
            "\"hasTrustDialogAccepted\": false",
            "\"hasTrustDialogAccepted\": true"
        )
    );
}

#[test]
fn an_entry_without_the_trust_key_gets_only_that_key() {
    let fixture = Fixture::new("");
    let config = format!(
        "{{\"projects\": {{{}: {{\"lastCost\": 1}}}}}}",
        serde_json::to_string(&fixture.key()).unwrap()
    );
    fs::write(fixture.config(), &config).unwrap();

    trust::record(
        &fixture.config(),
        &fixture.workspace(),
        &trust::Change::Trust(trust::McpApprovals::default()),
        trust::WhenMissing::Refuse,
    )
    .unwrap();

    assert_eq!(
        fixture.read(),
        config.replace(
            "{\"lastCost\"",
            "{\"hasTrustDialogAccepted\":true,\"lastCost\""
        )
    );
}

#[test]
fn a_config_without_projects_gets_the_projects_member() {
    let fixture = Fixture::new("{\"numStartups\": 1}");

    trust::record(
        &fixture.config(),
        &fixture.workspace(),
        &trust::Change::Trust(trust::McpApprovals::default()),
        trust::WhenMissing::Refuse,
    )
    .unwrap();

    assert_eq!(
        fixture.trust_of(&fixture.key()),
        Some(serde_json::Value::Bool(true))
    );
    let config: serde_json::Value = serde_json::from_str(&fixture.read()).unwrap();
    assert_eq!(config["numStartups"], 1);
}

#[test]
fn an_already_trusted_clone_leaves_the_file_untouched() {
    let fixture = Fixture::new("");
    let config = CONFIG.replace("/home/owner/other", &fixture.key());
    fs::write(fixture.config(), &config).unwrap();
    let before = fs::metadata(fixture.config()).unwrap().modified().unwrap();

    let recorded = trust::record(
        &fixture.config(),
        &fixture.workspace(),
        &trust::Change::Trust(trust::McpApprovals::default()),
        trust::WhenMissing::Refuse,
    )
    .unwrap();

    assert_eq!(recorded, Recorded::Unchanged);
    assert_eq!(fixture.read(), config);
    assert_eq!(
        fs::metadata(fixture.config()).unwrap().modified().unwrap(),
        before
    );
}

#[test]
fn the_write_keeps_the_files_mode_and_releases_the_lock() {
    let fixture = Fixture::new(CONFIG);

    trust::record(
        &fixture.config(),
        &fixture.workspace(),
        &trust::Change::Trust(trust::McpApprovals::default()),
        trust::WhenMissing::Refuse,
    )
    .unwrap();

    let mode = fs::metadata(fixture.config()).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
    assert!(!fixture.dir.path().join(".claude.json.lock").exists());
    let leftovers: Vec<_> = fs::read_dir(fixture.dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .filter(|name| name.to_string_lossy().contains("torlek"))
        .collect();
    assert_eq!(leftovers, Vec::<OsString>::new());
}

#[test]
fn a_config_that_cannot_be_parsed_is_refused_and_left_as_it_is() {
    let fixture = Fixture::new("{\"projects\": {");

    let refused = trust::record(
        &fixture.config(),
        &fixture.workspace(),
        &trust::Change::Trust(trust::McpApprovals::default()),
        trust::WhenMissing::Refuse,
    )
    .unwrap_err();

    assert!(refused.starts_with("the config is not JSON"), "{refused}");
    assert_eq!(fixture.read(), "{\"projects\": {");
}

#[test]
fn a_project_named_twice_is_refused() {
    let fixture = Fixture::new("");
    let key = serde_json::to_string(&fixture.key()).unwrap();
    let config = format!("{{\"projects\": {{{key}: {{}}, {key}: {{}}}}}}");
    fs::write(fixture.config(), &config).unwrap();

    let refused = trust::record(
        &fixture.config(),
        &fixture.workspace(),
        &trust::Change::Trust(trust::McpApprovals::default()),
        trust::WhenMissing::Refuse,
    )
    .unwrap_err();

    assert!(refused.contains("twice"), "{refused}");
    assert_eq!(fixture.read(), config);
}

#[test]
fn a_config_held_locked_by_claude_code_is_refused() {
    let fixture = Fixture::new(CONFIG);
    fs::create_dir(fixture.dir.path().join(".claude.json.lock")).unwrap();

    let refused = trust::record(
        &fixture.config(),
        &fixture.workspace(),
        &trust::Change::Trust(trust::McpApprovals::default()),
        trust::WhenMissing::Refuse,
    )
    .unwrap_err();

    assert!(refused.contains("stayed locked"), "{refused}");
    assert_eq!(fixture.read(), CONFIG);
    assert!(fixture.dir.path().join(".claude.json.lock").is_dir());
}

#[test]
fn a_config_that_is_a_link_is_refused() {
    let fixture = Fixture::new(CONFIG);
    let target = fixture.dir.path().join("dotfiles.json");
    fs::rename(fixture.config(), &target).unwrap();
    std::os::unix::fs::symlink(&target, fixture.config()).unwrap();

    let refused = trust::record(
        &fixture.config(),
        &fixture.workspace(),
        &trust::Change::Trust(trust::McpApprovals::default()),
        trust::WhenMissing::Refuse,
    )
    .unwrap_err();

    assert!(refused.contains("not a plain file"), "{refused}");
    assert_eq!(fs::read_to_string(&target).unwrap(), CONFIG);
}

#[test]
fn revoking_turns_only_the_clones_existing_trust_false() {
    let fixture = Fixture::new("");
    let config = CONFIG.replace("/home/owner/other", &fixture.key());
    fs::write(fixture.config(), &config).unwrap();

    let revoked = trust::record(
        &fixture.config(),
        &fixture.workspace(),
        &trust::Change::Revoke,
        trust::WhenMissing::Refuse,
    )
    .unwrap();

    assert_eq!(revoked, Recorded::Written);
    assert_eq!(
        fixture.read(),
        config.replace(
            "\"hasTrustDialogAccepted\": true",
            "\"hasTrustDialogAccepted\": false"
        )
    );
}

#[test]
fn revoking_a_clone_never_trusted_adds_nothing() {
    let fixture = Fixture::new(CONFIG);

    let revoked = trust::record(
        &fixture.config(),
        &fixture.workspace(),
        &trust::Change::Revoke,
        trust::WhenMissing::Refuse,
    )
    .unwrap();

    assert_eq!(revoked, Recorded::Unchanged);
    assert_eq!(fixture.read(), CONFIG);
}

fn checkouts() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join("main");
    let clone = dir.path().join("clone");
    for root in [&main, &clone] {
        fs::create_dir_all(root.join(".claude")).unwrap();
        fs::write(root.join(".claude/settings.json"), "{\"hooks\": {}}").unwrap();
    }
    (dir, main, clone)
}

fn unexpected(clone: &Path, main: &Path) -> Vec<String> {
    trust::unexpected_settings(clone, main, clone)
        .unwrap()
        .iter()
        .map(|path| path.display().to_string())
        .collect()
}

#[test]
fn a_clone_with_the_trusted_checkouts_settings_brings_nothing_unexpected() {
    let (_dir, main, clone) = checkouts();
    fs::write(main.join(".claude/settings.local.json"), "{}").unwrap();

    assert_eq!(unexpected(&clone, &main), Vec::<String>::new());
}

#[test]
fn a_clone_whose_settings_changed_is_named() {
    let (_dir, main, clone) = checkouts();
    fs::write(
        clone.join(".claude/settings.json"),
        "{\"hooks\": {\"SessionStart\": []}}",
    )
    .unwrap();

    assert_eq!(unexpected(&clone, &main), vec![".claude/settings.json"]);
}

#[test]
fn a_clone_with_settings_the_trusted_checkout_lacks_is_named() {
    let (_dir, main, clone) = checkouts();
    fs::write(clone.join(".mcp.json"), "{}").unwrap();
    fs::create_dir(clone.join(".claude/agents")).unwrap();
    fs::write(clone.join(".claude/agents/helper.md"), "x").unwrap();

    assert_eq!(
        unexpected(&clone, &main),
        vec![".claude/agents/helper.md", ".mcp.json"]
    );
}

#[test]
fn a_settings_link_in_the_clone_is_named() {
    let (_dir, main, clone) = checkouts();
    std::os::unix::fs::symlink("/etc/passwd", clone.join(".mcp.json")).unwrap();
    std::os::unix::fs::symlink("/etc/passwd", main.join(".mcp.json")).unwrap();

    assert_eq!(unexpected(&clone, &main), vec![".mcp.json"]);
}

#[test]
fn settings_where_the_agent_starts_below_the_clones_root_are_compared() {
    let (_dir, main, clone) = checkouts();
    fs::create_dir_all(clone.join("app/.claude")).unwrap();
    fs::write(clone.join("app/.mcp.json"), "{}").unwrap();
    fs::write(clone.join("app/.claude/settings.json"), "{}").unwrap();

    let unexpected: Vec<PathBuf> =
        trust::unexpected_settings(&clone, &main, &clone.join("app")).unwrap();

    assert_eq!(
        unexpected,
        vec![
            PathBuf::from("app/.claude/settings.json"),
            PathBuf::from("app/.mcp.json")
        ]
    );
}

fn environment(pairs: &[(&str, &Path)]) -> Vec<(OsString, OsString)> {
    pairs
        .iter()
        .map(|(name, value)| (OsString::from(name), value.as_os_str().to_owned()))
        .collect()
}

#[test]
fn the_config_file_follows_claude_config_dir_then_home() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let config_dir = dir.path().join("config");

    assert_eq!(
        trust::config_file(&environment(&[("HOME", &home)])).unwrap(),
        home.join(".claude.json")
    );
    assert_eq!(
        trust::config_file(&environment(&[
            ("HOME", &home),
            ("CLAUDE_CONFIG_DIR", &config_dir)
        ]))
        .unwrap(),
        config_dir.join(".claude.json")
    );
}

#[test]
fn an_older_config_claude_code_still_reads_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().to_owned();
    fs::create_dir(home.join(".claude")).unwrap();
    fs::write(home.join(".claude/.config.json"), "{}").unwrap();

    let refused = trust::config_file(&environment(&[("HOME", &home)])).unwrap_err();

    assert!(refused.contains(".config.json"), "{refused}");
}

/// A home with a fixture config, and a main checkout and a clone with the same settings.
fn adapter_fixture(config: impl Fn(&str) -> String) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let (dir, main, clone) = checkouts();
    let key = clone.canonicalize().unwrap().to_str().unwrap().to_owned();
    fs::write(dir.path().join(".claude.json"), config(&key)).unwrap();
    (dir, main, clone)
}

fn trust_through_the_adapter(
    home: &Path,
    main: &Path,
    clone: &Path,
) -> Result<WorkspaceTrust, String> {
    let claude = ClaudeCode::new(PathBuf::from("/usr/bin/claude"), None, None);
    claude
        .trust_workspace(&TrustRequest {
            workspace: clone,
            working_dir: clone,
            trusted_checkout: main,
            default_branch: None,
            environment: &environment(&[("HOME", home)]),
            session_home: None,
        })
        .map_err(|error| error.reason)
}

#[test]
fn a_clone_with_its_trusted_checkouts_settings_is_trusted_through_the_adapter() {
    let (dir, main, clone) = adapter_fixture(|_| CONFIG.to_owned());

    let trusted = trust_through_the_adapter(dir.path(), &main, &clone);

    assert_eq!(trusted, Ok(WorkspaceTrust::Recorded { notice: None }));
}

#[test]
fn a_reused_clone_whose_settings_changed_loses_the_trust_an_earlier_launch_recorded() {
    let (dir, main, clone) = adapter_fixture(|key| CONFIG.replace("/home/owner/other", key));
    fs::write(clone.join(".mcp.json"), "{\"mcpServers\": {}}").unwrap();

    let refused = trust_through_the_adapter(dir.path(), &main, &clone).unwrap_err();

    assert!(refused.contains(".mcp.json"), "{refused}");
    assert!(refused.contains("revoked"), "{refused}");
    let config: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(dir.path().join(".claude.json")).unwrap())
            .unwrap();
    let key = clone.canonicalize().unwrap();
    assert_eq!(
        config["projects"][key.to_str().unwrap()]["hasTrustDialogAccepted"],
        false
    );
}

/// #968: a sandboxed session's home holds no config before its first Claude Code runs, so
/// the trust is written into a new owner-only file holding it alone.
#[test]
fn a_missing_config_the_caller_may_create_is_created_owner_only_with_the_trust() {
    let fixture = Fixture::new(CONFIG);
    fs::remove_file(fixture.config()).unwrap();

    let recorded = trust::record(
        &fixture.config(),
        &fixture.workspace(),
        &trust::Change::Trust(trust::McpApprovals::default()),
        trust::WhenMissing::Create,
    )
    .unwrap();

    assert_eq!(recorded, Recorded::Written);
    assert_eq!(fixture.trust_of(&fixture.key()), Some(true.into()));
    let mode = fs::metadata(fixture.config()).unwrap().permissions().mode() & 0o7777;
    assert_eq!(mode, 0o600);
}

#[test]
fn a_missing_config_is_refused_unless_the_caller_may_create_it() {
    let fixture = Fixture::new(CONFIG);
    fs::remove_file(fixture.config()).unwrap();

    let refused = trust::record(
        &fixture.config(),
        &fixture.workspace(),
        &trust::Change::Trust(trust::McpApprovals::default()),
        trust::WhenMissing::Refuse,
    )
    .unwrap_err();

    assert!(refused.starts_with("read "), "{refused}");
    assert!(!fixture.config().exists());
}

#[test]
fn revoking_in_a_missing_config_creates_nothing() {
    let fixture = Fixture::new(CONFIG);
    fs::remove_file(fixture.config()).unwrap();

    let recorded = trust::record(
        &fixture.config(),
        &fixture.workspace(),
        &trust::Change::Revoke,
        trust::WhenMissing::Create,
    )
    .unwrap();

    assert_eq!(recorded, Recorded::Unchanged);
    assert!(!fixture.config().exists());
}

/// #968: a sandboxed agent's `~/.claude.json` lies in its session's home; the owner's own file
/// is never touched.
#[test]
fn a_sandboxed_launch_records_the_trust_in_the_sessions_home_not_the_owners() {
    let (dir, main, clone) = adapter_fixture(|_| CONFIG.to_owned());
    let session_home = dir.path().join("session-home");
    fs::create_dir(&session_home).unwrap();
    let claude = ClaudeCode::new(PathBuf::from("/usr/bin/claude"), None, None);

    let trusted = claude.trust_workspace(&TrustRequest {
        workspace: &clone,
        working_dir: &clone,
        trusted_checkout: &main,
        default_branch: None,
        environment: &environment(&[("HOME", dir.path())]),
        session_home: Some(&session_home),
    });

    assert_eq!(
        trusted.map_err(|error| error.reason),
        Ok(WorkspaceTrust::Recorded { notice: None })
    );
    assert_eq!(
        fs::read_to_string(dir.path().join(".claude.json")).unwrap(),
        CONFIG
    );
    let written: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(session_home.join(".claude.json")).unwrap())
            .unwrap();
    let key = clone.canonicalize().unwrap();
    assert_eq!(
        written["projects"][key.to_str().unwrap()]["hasTrustDialogAccepted"],
        true
    );
}

#[test]
fn a_session_config_is_refused_when_claude_config_dir_names_another_folder() {
    let dir = tempfile::tempdir().unwrap();

    let refused = trust::session_config_file(
        &environment(&[("HOME", dir.path()), ("CLAUDE_CONFIG_DIR", dir.path())]),
        &dir.path().join("session-home"),
    )
    .unwrap_err();

    assert!(refused.contains("CLAUDE_CONFIG_DIR"), "{refused}");
}

/// Issue #1279: the default branch as the trusted checkout fetched it, as the files it holds.
const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

fn trust_with_default_branch(
    home: &Path,
    main: &Path,
    clone: &Path,
    files: &[(&str, &str)],
) -> Result<WorkspaceTrust, String> {
    let files: Vec<(PathBuf, Vec<u8>)> = files
        .iter()
        .map(|(path, text)| (PathBuf::from(path), text.as_bytes().to_vec()))
        .collect();
    let file = |relative: &Path| {
        Ok(files
            .iter()
            .find(|(path, _)| path == relative)
            .map(|(_, bytes)| bytes.clone()))
    };
    let default_branch = DefaultBranch {
        commit: COMMIT,
        file: &file,
    };
    ClaudeCode::new(PathBuf::from("/usr/bin/claude"), None, None)
        .trust_workspace(&TrustRequest {
            workspace: clone,
            working_dir: clone,
            trusted_checkout: main,
            default_branch: Some(&default_branch),
            environment: &environment(&[("HOME", home)]),
            session_home: None,
        })
        .map_err(|error| error.reason)
}

/// The clone's entry in the fixture home's config.
fn clone_entry(home: &Path, clone: &Path) -> serde_json::Value {
    let config: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(home.join(".claude.json")).unwrap()).unwrap();
    config["projects"][clone.canonicalize().unwrap().to_str().unwrap()].clone()
}

const SETTINGS: &str = "{\"hooks\": {}}";
const DOCS_SERVER: &str = r#"{"mcpServers": {"docs": {"command": "docs-mcp"}}}"#;
const DOCS_AND_EXTRA: &str =
    r#"{"mcpServers": {"docs": {"command": "docs-mcp"}, "extra": {"command": "extra-mcp"}}}"#;

/// A home whose config approves `approvals` (members of the trusted checkout's entry) for the
/// main checkout, and a clone bringing `clone_mcp` as its `.mcp.json`.
fn mcp_fixture(
    main_mcp: Option<&str>,
    clone_mcp: &str,
    approvals: &str,
) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let (dir, main, clone) = checkouts();
    if let Some(main_mcp) = main_mcp {
        fs::write(main.join(".mcp.json"), main_mcp).unwrap();
    }
    fs::write(clone.join(".mcp.json"), clone_mcp).unwrap();
    let main_key = main.canonicalize().unwrap().to_str().unwrap().to_owned();
    fs::write(
        dir.path().join(".claude.json"),
        format!(
            "{{\"projects\": {{{}: {{\"hasTrustDialogAccepted\": true{approvals}}}}}}}",
            serde_json::to_string(&main_key).unwrap()
        ),
    )
    .unwrap();
    (dir, main, clone)
}

/// Option B of #1279: the owner's pull, not a merge on the remote, accepts settings.
#[test]
fn a_clone_of_settings_merged_while_the_checkout_is_behind_is_refused_naming_the_pull() {
    let (dir, main, clone) = adapter_fixture(|_| CONFIG.to_owned());
    fs::write(clone.join(".mcp.json"), "{}").unwrap();

    let refused = trust_with_default_branch(
        dir.path(),
        &main,
        &clone,
        &[(".claude/settings.json", SETTINGS), (".mcp.json", "{}")],
    )
    .unwrap_err();

    assert_eq!(
        refused,
        format!(
            "run `git -C {} pull --ff-only`: the trusted checkout is behind its origin/HEAD at \
             0123456789ab, which holds this clone's settings (.mcp.json)",
            main.display()
        )
    );
    assert_eq!(clone_entry(dir.path(), &clone), serde_json::Value::Null);
}

#[test]
fn settings_the_default_branch_lacks_too_are_refused_naming_each_file() {
    let (dir, main, clone) = adapter_fixture(|_| CONFIG.to_owned());
    fs::write(clone.join(".mcp.json"), "{\"mcpServers\": {}}").unwrap();

    let refused = trust_with_default_branch(
        dir.path(),
        &main,
        &clone,
        &[(".claude/settings.json", SETTINGS), (".mcp.json", "{}")],
    )
    .unwrap_err();

    assert_eq!(
        refused,
        "its harness settings differ from the trusted checkout's working tree (.mcp.json)"
    );
    assert_eq!(clone_entry(dir.path(), &clone), serde_json::Value::Null);
}

/// Each file matches one reference, but no reference holds them all: no pull would help.
#[test]
fn settings_mixing_the_working_tree_and_the_default_branch_name_no_pull() {
    let (dir, main, clone) = adapter_fixture(|_| CONFIG.to_owned());
    fs::write(clone.join(".mcp.json"), "{}").unwrap();

    let refused = trust_with_default_branch(
        dir.path(),
        &main,
        &clone,
        &[(".claude/settings.json", "{}"), (".mcp.json", "{}")],
    )
    .unwrap_err();

    assert_eq!(
        refused,
        "its harness settings differ from the trusted checkout's working tree (.mcp.json)"
    );
}

#[test]
fn a_checkout_path_needing_quotes_is_quoted_in_the_pull_command() {
    let (dir, _main, clone) = adapter_fixture(|_| CONFIG.to_owned());
    let main = dir.path().join("owner's checkout");
    fs::create_dir_all(main.join(".claude")).unwrap();
    fs::write(main.join(".claude/settings.json"), SETTINGS).unwrap();
    fs::write(clone.join(".mcp.json"), "{}").unwrap();

    let refused = trust_with_default_branch(
        dir.path(),
        &main,
        &clone,
        &[(".claude/settings.json", SETTINGS), (".mcp.json", "{}")],
    )
    .unwrap_err();

    assert!(
        refused.starts_with(&format!(
            "run `git -C '{}' pull --ff-only`",
            main.display().to_string().replace('\'', "'\\''")
        )),
        "{refused}"
    );
}

#[test]
fn the_owners_approved_servers_carry_over_to_a_clone_that_defines_them_alike() {
    let (dir, main, clone) = mcp_fixture(
        Some(DOCS_SERVER),
        DOCS_SERVER,
        r#", "enabledMcpjsonServers": ["docs"], "disabledMcpjsonServers": ["old"]"#,
    );

    let trusted = trust_through_the_adapter(dir.path(), &main, &clone).unwrap();

    assert_eq!(trusted, WorkspaceTrust::Recorded { notice: None });
    let entry = clone_entry(dir.path(), &clone);
    assert_eq!(entry["enabledMcpjsonServers"], serde_json::json!(["docs"]));
    assert_eq!(entry["disabledMcpjsonServers"], serde_json::json!(["old"]));
}

#[test]
fn a_server_the_owner_never_approved_is_not_enabled_and_is_named() {
    let (dir, main, clone) = mcp_fixture(
        Some(DOCS_AND_EXTRA),
        DOCS_AND_EXTRA,
        r#", "enabledMcpjsonServers": ["docs"]"#,
    );

    let trusted = trust_through_the_adapter(dir.path(), &main, &clone).unwrap();

    assert_eq!(
        trusted.notice(),
        Some("MCP servers left to Claude Code's prompt: extra")
    );
    let entry = clone_entry(dir.path(), &clone);
    assert_eq!(entry["enabledMcpjsonServers"], serde_json::json!(["docs"]));
}

/// The settings check refuses such a clone first; the carry-over keeps its own guard.
#[test]
fn a_server_the_clone_defines_otherwise_than_the_checkout_waits_on_the_prompt() {
    let redefined = r#"{"mcpServers": {"docs": {"command": "other-mcp"}}}"#;
    let (dir, main, clone) = mcp_fixture(
        Some(DOCS_SERVER),
        redefined,
        r#", "enabledMcpjsonServers": ["docs"]"#,
    );

    let carried =
        trust::carried_approvals(&dir.path().join(".claude.json"), &clone, &main, &clone).unwrap();

    assert_eq!(carried.approvals, trust::McpApprovals::default());
    assert_eq!(carried.waiting, ["docs"]);
}

/// "Use all of this project's servers" in the checkout approves each server it defines, by
/// name.
#[test]
fn approving_all_servers_in_the_checkout_carries_each_one_by_name() {
    let (dir, main, clone) = mcp_fixture(
        Some(DOCS_AND_EXTRA),
        DOCS_AND_EXTRA,
        r#", "enableAllProjectMcpServers": true"#,
    );

    trust_through_the_adapter(dir.path(), &main, &clone).unwrap();

    let entry = clone_entry(dir.path(), &clone);
    assert_eq!(
        entry["enabledMcpjsonServers"],
        serde_json::json!(["docs", "extra"])
    );
    assert_eq!(entry["enableAllProjectMcpServers"], serde_json::Value::Null);
}

/// A reused clone whose next branch is refused keeps no approval an earlier launch carried.
#[test]
fn revoking_clears_the_clones_project_server_approvals() {
    let fixture = Fixture::new("");
    let config = format!(
        "{{\"projects\": {{{}: {{\"hasTrustDialogAccepted\": true, \
         \"enabledMcpjsonServers\": [\"docs\"], \"enableAllProjectMcpServers\": true}}}}}}",
        serde_json::to_string(&fixture.key()).unwrap()
    );
    fs::write(fixture.config(), &config).unwrap();

    trust::record(
        &fixture.config(),
        &fixture.workspace(),
        &trust::Change::Revoke,
        trust::WhenMissing::Refuse,
    )
    .unwrap();

    assert_eq!(
        fixture.read(),
        config
            .replace(
                "\"hasTrustDialogAccepted\": true",
                "\"hasTrustDialogAccepted\": false"
            )
            .replace("[\"docs\"]", "[]")
            .replace(
                "\"enableAllProjectMcpServers\": true",
                "\"enableAllProjectMcpServers\": false"
            )
    );
}

/// A server the owner rejected in the clone itself stays rejected beside the checkout's.
#[test]
fn the_clones_own_rejections_stay_beside_the_carried_ones() {
    let fixture = Fixture::new("");
    let config = format!(
        "{{\"projects\": {{{}: {{\"hasTrustDialogAccepted\": true, \
         \"disabledMcpjsonServers\": [\"mine\"]}}}}}}",
        serde_json::to_string(&fixture.key()).unwrap()
    );
    fs::write(fixture.config(), &config).unwrap();

    trust::record(
        &fixture.config(),
        &fixture.workspace(),
        &trust::Change::Trust(trust::McpApprovals {
            enabled: Vec::new(),
            disabled: vec!["old".to_owned(), "mine".to_owned()],
        }),
        trust::WhenMissing::Refuse,
    )
    .unwrap();

    assert_eq!(
        fixture.read(),
        config.replace("[\"mine\"]", "[\"mine\",\"old\"]")
    );
}
