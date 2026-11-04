//! Issue #702: Claude Code's folder trust recorded for a parallel clone, against fixture
//! config files only; the owner's own `~/.claude.json` is never read.

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use cockpit_claude::ClaudeCode;
use cockpit_claude::trust::{self, Recorded};
use cockpit_provider_api::{Provider, TrustRequest, WorkspaceTrust};

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

    let recorded = trust::record(&fixture.config(), &fixture.workspace(), true).unwrap();

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

    trust::record(&fixture.config(), &fixture.workspace(), true).unwrap();

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

    trust::record(&fixture.config(), &fixture.workspace(), true).unwrap();

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

    trust::record(&fixture.config(), &fixture.workspace(), true).unwrap();

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

    let recorded = trust::record(&fixture.config(), &fixture.workspace(), true).unwrap();

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

    trust::record(&fixture.config(), &fixture.workspace(), true).unwrap();

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

    let refused = trust::record(&fixture.config(), &fixture.workspace(), true).unwrap_err();

    assert!(refused.starts_with("the config is not JSON"), "{refused}");
    assert_eq!(fixture.read(), "{\"projects\": {");
}

#[test]
fn a_project_named_twice_is_refused() {
    let fixture = Fixture::new("");
    let key = serde_json::to_string(&fixture.key()).unwrap();
    let config = format!("{{\"projects\": {{{key}: {{}}, {key}: {{}}}}}}");
    fs::write(fixture.config(), &config).unwrap();

    let refused = trust::record(&fixture.config(), &fixture.workspace(), true).unwrap_err();

    assert!(refused.contains("twice"), "{refused}");
    assert_eq!(fixture.read(), config);
}

#[test]
fn a_config_held_locked_by_claude_code_is_refused() {
    let fixture = Fixture::new(CONFIG);
    fs::create_dir(fixture.dir.path().join(".claude.json.lock")).unwrap();

    let refused = trust::record(&fixture.config(), &fixture.workspace(), true).unwrap_err();

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

    let refused = trust::record(&fixture.config(), &fixture.workspace(), true).unwrap_err();

    assert!(refused.contains("not a plain file"), "{refused}");
    assert_eq!(fs::read_to_string(&target).unwrap(), CONFIG);
}

#[test]
fn revoking_turns_only_the_clones_existing_trust_false() {
    let fixture = Fixture::new("");
    let config = CONFIG.replace("/home/owner/other", &fixture.key());
    fs::write(fixture.config(), &config).unwrap();

    let revoked = trust::record(&fixture.config(), &fixture.workspace(), false).unwrap();

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

    let revoked = trust::record(&fixture.config(), &fixture.workspace(), false).unwrap();

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
            environment: &environment(&[("HOME", home)]),
        })
        .map_err(|error| error.reason)
}

#[test]
fn a_clone_with_its_trusted_checkouts_settings_is_trusted_through_the_adapter() {
    let (dir, main, clone) = adapter_fixture(|_| CONFIG.to_owned());

    let trusted = trust_through_the_adapter(dir.path(), &main, &clone);

    assert_eq!(trusted, Ok(WorkspaceTrust::Recorded));
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
