//! Issue #1178: Claude Code's first-run screens answered in a sandboxed session's own home,
//! against fixture homes only; the owner's own `~/.claude.json` is never read.

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use cockpit_claude::ClaudeCode;
use cockpit_claude::first_run::{self, key_tail};
use cockpit_claude::trust::Recorded;
use cockpit_provider_api::{
    HeldCredential, Provider, SessionHomeRequest, TrustRequest, WorkspaceTrust,
};
use serde_json::{Value, json};

/// What the sandbox gives the agent in its credential variable; a fixture, never a key.
const PLACEHOLDER: &str = "fixture-placeholder-the-session-proxy-adds-the-credential";

/// A session home's config as Claude Code 2.1.319 leaves one after a start stopped on its
/// onboarding: two-space indents, its own keys, no onboarding flag.
const STOPPED_ON_ONBOARDING: &str = r#"{
  "numStartups": 1,
  "projects": {
    "/home/redteam/redteam-project": {
      "hasTrustDialogAccepted": true
    }
  },
  "userID": "abc\"def"
}
"#;

struct SessionHome {
    dir: tempfile::TempDir,
}

impl SessionHome {
    fn new() -> SessionHome {
        SessionHome {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn with_config(config: &str) -> SessionHome {
        let home = SessionHome::new();
        fs::write(home.config(), config).unwrap();
        fs::set_permissions(home.config(), fs::Permissions::from_mode(0o600)).unwrap();
        home
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn config(&self) -> PathBuf {
        self.path().join(".claude.json")
    }

    fn read(&self) -> String {
        fs::read_to_string(self.config()).unwrap()
    }

    fn parsed(&self) -> Value {
        serde_json::from_str(&self.read()).unwrap()
    }

    /// The adapter's `prepare_session_home`, as the daemon calls it before a sandboxed launch,
    /// with `api_key_placeholder` held in `ANTHROPIC_API_KEY` when given.
    fn prepare(&self, api_key_placeholder: Option<&str>) -> Result<(), String> {
        let credential = api_key_placeholder.map(|placeholder| HeldCredential {
            variable: "ANTHROPIC_API_KEY",
            placeholder,
        });
        self.prepare_with(&[], credential)
    }

    fn prepare_with(
        &self,
        extra: &[(&str, &Path)],
        credential: Option<HeldCredential<'_>>,
    ) -> Result<(), String> {
        let owner_home = self.path().join("owner");
        let mut environment = vec![(OsString::from("HOME"), owner_home.into_os_string())];
        environment.extend(
            extra
                .iter()
                .map(|(name, value)| (OsString::from(name), value.as_os_str().to_owned())),
        );
        claude()
            .prepare_session_home(&SessionHomeRequest {
                environment: &environment,
                session_home: self.path(),
                credential,
            })
            .map_err(|error| error.reason)
    }
}

fn claude() -> ClaudeCode {
    ClaudeCode::new(PathBuf::from("/usr/bin/claude"), None, None)
}

#[test]
fn a_fresh_session_home_starts_past_the_onboarding() {
    let home = SessionHome::new();

    home.prepare(None).unwrap();

    assert_eq!(home.parsed(), json!({ "hasCompletedOnboarding": true }));
    let mode = fs::metadata(home.config()).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

#[test]
fn a_bound_credential_answers_yes_to_the_key_the_agent_holds() {
    let home = SessionHome::new();

    home.prepare(Some(PLACEHOLDER)).unwrap();

    assert_eq!(
        home.parsed()["customApiKeyResponses"],
        json!({ "approved": [key_tail(PLACEHOLDER)], "rejected": [] })
    );
}

/// Claude Code records an answer under `key.trim().slice(-20)`.
#[test]
fn the_key_tail_is_the_last_twenty_characters_of_the_trimmed_key() {
    assert_eq!(
        key_tail("  sk-ant-api03-0123456789abcdefghijKLMNOP\n"),
        "6789abcdefghijKLMNOP"
    );
    assert_eq!(key_tail("short"), "short");
}

#[test]
fn a_home_a_start_stopped_on_the_onboarding_keeps_every_other_byte() {
    let home = SessionHome::with_config(STOPPED_ON_ONBOARDING);

    home.prepare(None).unwrap();

    assert_eq!(
        home.read(),
        STOPPED_ON_ONBOARDING.replacen('{', "{\"hasCompletedOnboarding\":true,", 1)
    );
}

#[test]
fn an_onboarding_flag_left_false_is_turned_true_in_place() {
    let home = SessionHome::with_config("{\"hasCompletedOnboarding\": false, \"theme\": \"dark\"}");

    home.prepare(None).unwrap();

    assert_eq!(
        home.read(),
        "{\"hasCompletedOnboarding\": true, \"theme\": \"dark\"}"
    );
}

#[test]
fn a_home_already_answered_is_left_untouched() {
    let tail = key_tail(PLACEHOLDER);
    let config = format!(
        "{{\"hasCompletedOnboarding\":true,\"customApiKeyResponses\":{{\"approved\":[\"{tail}\"]}}}}"
    );

    let recorded = first_run::answered(&config, Some(&tail)).unwrap();

    assert_eq!(recorded, None);
}

/// A "no" given at the terminal of this session is the owner's answer and stays.
#[test]
fn a_no_to_the_key_is_kept() {
    let tail = key_tail(PLACEHOLDER);
    let config = format!(
        "{{\"hasCompletedOnboarding\":true,\"customApiKeyResponses\":{{\"approved\":[],\"rejected\":[\"{tail}\"]}}}}"
    );
    let home = SessionHome::with_config(&config);

    home.prepare(Some(PLACEHOLDER)).unwrap();

    assert_eq!(home.read(), config);
}

#[test]
fn answers_about_another_key_gain_a_yes_for_the_key_the_agent_holds() {
    let home = SessionHome::with_config(
        "{\"customApiKeyResponses\":{\"approved\":[\"another-key-tail-xyz\"],\"rejected\":[\"a-third-key-tail-xyz\"]}}",
    );

    home.prepare(Some(PLACEHOLDER)).unwrap();

    let config = home.parsed();
    assert_eq!(config["hasCompletedOnboarding"], true);
    assert_eq!(
        config["customApiKeyResponses"],
        json!({
            "approved": ["another-key-tail-xyz", key_tail(PLACEHOLDER)],
            "rejected": ["a-third-key-tail-xyz"],
        })
    );
}

/// Claude Code reads a null where answers go as no answers, and asks.
#[test]
fn null_answers_gain_a_yes_for_the_key_the_agent_holds() {
    let home = SessionHome::with_config("{\"customApiKeyResponses\":{\"approved\":null}}");

    home.prepare(Some(PLACEHOLDER)).unwrap();

    assert_eq!(
        home.parsed()["customApiKeyResponses"],
        json!({ "approved": [key_tail(PLACEHOLDER)], "rejected": [] })
    );
}

/// Claude Code asks about `ANTHROPIC_API_KEY` alone; a `setup-token` token gets no answer.
#[test]
fn an_oauth_token_records_no_answer_about_an_api_key() {
    let home = SessionHome::new();

    home.prepare_with(
        &[],
        Some(HeldCredential {
            variable: "CLAUDE_CODE_OAUTH_TOKEN",
            placeholder: PLACEHOLDER,
        }),
    )
    .unwrap();

    assert_eq!(home.parsed(), json!({ "hasCompletedOnboarding": true }));
}

#[test]
fn answers_that_are_not_an_object_are_refused_writing_nothing() {
    let config = "{\"customApiKeyResponses\":[\"not-answers\"]}";
    let home = SessionHome::with_config(config);

    let refused = home.prepare(Some(PLACEHOLDER)).unwrap_err();

    assert!(refused.contains("customApiKeyResponses"), "{refused}");
    assert_eq!(home.read(), config);
}

#[test]
fn a_config_that_is_a_link_is_refused_writing_nothing() {
    let home = SessionHome::new();
    let elsewhere = home.path().join("elsewhere.json");
    fs::write(&elsewhere, "{}").unwrap();
    std::os::unix::fs::symlink(&elsewhere, home.config()).unwrap();

    let refused = home.prepare(None).unwrap_err();

    assert!(refused.contains("not a plain file"), "{refused}");
    assert_eq!(fs::read_to_string(&elsewhere).unwrap(), "{}");
}

#[test]
fn a_config_that_is_not_json_is_refused_writing_nothing() {
    let home = SessionHome::with_config("{\"numStartups\": ");

    let refused = home.prepare(None).unwrap_err();

    assert!(refused.contains("not JSON"), "{refused}");
    assert_eq!(home.read(), "{\"numStartups\": ");
}

#[test]
fn a_launch_naming_claude_config_dir_is_refused_writing_nothing() {
    let home = SessionHome::new();
    let config_dir = home.path().join("config-dir");

    let refused = home
        .prepare_with(&[("CLAUDE_CONFIG_DIR", &config_dir)], None)
        .unwrap_err();

    assert!(refused.contains("CLAUDE_CONFIG_DIR"), "{refused}");
    assert!(!home.config().exists());
}

/// The daemon prepares the home, then records a trusted project's trust in the same file.
#[test]
fn the_folder_trust_recorded_after_keeps_the_answers() {
    let home = SessionHome::new();
    let checkout = home.path().join("checkout");
    fs::create_dir(&checkout).unwrap();
    home.prepare(Some(PLACEHOLDER)).unwrap();

    let trusted = claude().trust_workspace(&TrustRequest {
        workspace: &checkout,
        working_dir: &checkout,
        trusted_checkout: &checkout,
        default_branch: None,
        environment: &[(OsString::from("HOME"), home.path().join("owner").into())],
        session_home: Some(home.path()),
    });

    assert_eq!(
        trusted.map_err(|error| error.reason),
        Ok(WorkspaceTrust::Recorded { notice: None })
    );
    let config = home.parsed();
    let key = checkout.canonicalize().unwrap();
    assert_eq!(
        config["projects"][key.to_str().unwrap()]["hasTrustDialogAccepted"],
        true
    );
    assert_eq!(config["hasCompletedOnboarding"], true);
    assert_eq!(
        config["customApiKeyResponses"]["approved"],
        json!([key_tail(PLACEHOLDER)])
    );
}

#[test]
fn a_second_launch_finds_nothing_to_record() {
    let home = SessionHome::new();
    home.prepare(Some(PLACEHOLDER)).unwrap();
    let first = home.read();

    let recorded = first_run::record(&home.config(), Some(PLACEHOLDER)).unwrap();

    assert_eq!(recorded, Recorded::Unchanged);
    assert_eq!(home.read(), first);
}
