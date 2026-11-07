//! Plan windows through the public provider contract, against a scratch home and a fake
//! `codex app-server` that answers like 0.166.0 does: never the network, the owner's own
//! sign-in or the sessions' `~/.codex`.
mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use cockpit_codex::{CONFORMANCE_VERSION, Codex};
use cockpit_provider_api::{Provider, QuotaReading};
use serde_json::{Value, json};

const NOW: i64 = 1_800_000_000;
const HOUR: i64 = 3600;

struct Home {
    dir: tempfile::TempDir,
}

impl Home {
    /// A home with the quota sign-in's folder, as `codex-quota.py` leaves it.
    fn with_quota_home() -> Home {
        let home = Home::without_quota_home();
        std::fs::create_dir_all(home.quota_home()).unwrap();
        home
    }

    fn without_quota_home() -> Home {
        Home {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn quota_home(&self) -> PathBuf {
        self.path().join(".local/share/codex-quota")
    }

    fn record(&self, name: &str) -> Option<String> {
        std::fs::read_to_string(self.path().join(name)).ok()
    }

    /// A `codex` that records its words, `CODEX_HOME`, folder and pid beside the home,
    /// reads the four request lines, prints `answers` one per line, then runs `after`.
    fn codex_answering(&self, answers: &[Value], after: &str) -> Codex {
        let answers_file = self.path().join("answers");
        let lines: String = answers.iter().map(|answer| format!("{answer}\n")).collect();
        std::fs::write(&answers_file, lines).unwrap();
        let record = self.path().display();
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{record}/words'\n\
             printf '%s' \"$CODEX_HOME\" > '{record}/codex-home'\npwd > '{record}/folder'\n\
             echo $$ > '{record}/pid'\nenv > '{record}/environment'\n\
             for request in 1 2 3 4; do IFS= read -r line; printf '%s\\n' \"$line\" >> '{record}/requests'; done\n\
             cat '{}'\n{after}\n",
            answers_file.display()
        );
        let program = self.path().join("codex");
        std::fs::write(&program, script).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        Codex::new(program, Some(CONFORMANCE_VERSION.to_owned()), None)
    }

    /// Answers as a signed-in 0.166.0 does: a notification first, the reads out of order,
    /// then it exits once its input ends.
    fn codex(&self, account: Value, rate_limits: Value) -> Codex {
        self.codex_answering(&signed_in_answers(account, rate_limits), "cat > /dev/null")
    }
}

fn signed_in_answers(account: Value, rate_limits: Value) -> Vec<Value> {
    vec![
        json!({"id": 1, "result": {"userAgent": "torlek_quota/0.1.0"}}),
        json!({"method": "remoteControl/status/changed", "params": {"status": "disabled"}}),
        json!({"id": 3, "result": rate_limits}),
        json!({"id": 2, "result": {"account": account, "requiresOpenaiAuth": true}}),
    ]
}

fn subscription() -> Value {
    json!({"type": "chatgpt", "email": "owner@example.com", "planType": "pro", "workspaceName": null})
}

fn window(used_percent: f64, minutes: Option<u32>, resets_at: Option<i64>) -> Value {
    json!({"usedPercent": used_percent, "windowDurationMins": minutes, "resetsAt": resets_at})
}

fn bucket(primary: Value, secondary: Value) -> Value {
    json!({"limitId": "codex", "limitName": null, "primary": primary, "secondary": secondary,
           "credits": null, "planType": "pro", "rateLimitReachedType": null})
}

fn by_limit_id(codex: Value, ordinary_usage_allowed: Value) -> Value {
    json!({
        "ordinaryUsageAllowed": ordinary_usage_allowed,
        "rateLimits": codex.clone(),
        "rateLimitsByLimitId": {
            "codex": codex,
            "codex_other": bucket(window(99.0, Some(300), None), Value::Null),
        },
        "rateLimitResetCredits": null,
        "accountId": "account-private",
        "rateLimitUpsell": null,
    })
}

fn read(codex: &Codex, home: &Home) -> QuotaReading {
    codex.quota(home.path()).unwrap()
}

fn refusal(codex: &Codex, home: &Home) -> String {
    codex.quota(home.path()).unwrap_err().reason
}

/// limit, length in seconds, share used, reset.
type Window<'reading> = (&'reading str, Option<u32>, f64, Option<i64>);

fn windows(reading: &QuotaReading) -> Vec<Window<'_>> {
    reading
        .windows()
        .iter()
        .map(|window| {
            (
                window.limit().as_str(),
                window.length_seconds(),
                window.used_percent(),
                window.resets_at(),
            )
        })
        .collect()
}

#[test]
fn the_codex_buckets_windows_are_keyed_by_their_length() {
    let home = Home::with_quota_home();
    let codex = home.codex(
        subscription(),
        by_limit_id(
            bucket(
                window(42.0, Some(300), Some(NOW + 2 * HOUR)),
                window(81.5, Some(10_080), Some(NOW + 122 * HOUR)),
            ),
            json!(true),
        ),
    );

    let reading = read(&codex, &home);

    assert_eq!(
        windows(&reading),
        [
            ("five-hour", Some(18_000), 42.0, Some(NOW + 2 * HOUR)),
            ("seven-day", Some(604_800), 81.5, Some(NOW + 122 * HOUR)),
        ]
    );
    assert_eq!(reading.plan(), Some("pro"));
    assert!(!reading.is_blocked());
}

#[test]
fn the_older_single_bucket_answer_is_read_and_a_refused_use_is_blocked() {
    let home = Home::with_quota_home();
    let mut single = bucket(window(100.0, Some(300), None), Value::Null);
    single["limitId"] = Value::Null;
    let codex = home.codex(
        subscription(),
        json!({"ordinaryUsageAllowed": false, "rateLimits": single, "rateLimitsByLimitId": null}),
    );

    let reading = read(&codex, &home);

    assert_eq!(
        windows(&reading),
        [("five-hour", Some(18_000), 100.0, None)]
    );
    assert!(reading.is_blocked());
}

#[test]
fn an_unknown_whether_use_is_allowed_is_not_blocked() {
    let home = Home::with_quota_home();
    let codex = home.codex(
        subscription(),
        by_limit_id(
            bucket(window(5.0, Some(300), None), Value::Null),
            Value::Null,
        ),
    );

    assert!(!read(&codex, &home).is_blocked());
}

#[test]
fn a_window_of_another_length_or_none_keeps_a_name_of_its_own() {
    let home = Home::with_quota_home();
    let codex = home.codex(
        subscription(),
        by_limit_id(
            bucket(window(1.0, Some(1440), None), window(2.0, None, None)),
            json!(true),
        ),
    );

    let reading = read(&codex, &home);

    assert_eq!(
        windows(&reading),
        [
            ("window-1440-minutes", Some(86_400), 1.0, None),
            ("secondary", None, 2.0, None),
        ]
    );
}

#[test]
fn codex_runs_in_the_quota_sign_ins_home_never_the_sessions() {
    let home = Home::with_quota_home();
    let codex = home.codex(
        subscription(),
        by_limit_id(
            bucket(window(5.0, Some(300), None), Value::Null),
            json!(true),
        ),
    );

    read(&codex, &home);

    let quota_home = home.quota_home().display().to_string();
    assert_eq!(
        home.record("codex-home").as_deref(),
        Some(quota_home.as_str())
    );
    assert_eq!(
        home.record("folder").map(|folder| folder.trim().to_owned()),
        Some(quota_home)
    );
    assert_eq!(
        home.record("words").as_deref(),
        Some("app-server\n-c\ncli_auth_credentials_store=\"file\"\n")
    );
    assert!(!home.path().join(".codex").exists());
}

#[test]
fn it_asks_for_the_account_without_a_token_refresh_then_the_rate_limits() {
    let home = Home::with_quota_home();
    let codex = home.codex(
        subscription(),
        by_limit_id(
            bucket(window(5.0, Some(300), None), Value::Null),
            json!(true),
        ),
    );

    read(&codex, &home);

    let requests: Vec<Value> = home
        .record("requests")
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let methods: Vec<&str> = requests
        .iter()
        .map(|request| request["method"].as_str().unwrap())
        .collect();
    assert_eq!(
        methods,
        [
            "initialize",
            "initialized",
            "account/read",
            "account/rateLimits/read"
        ]
    );
    assert_eq!(requests[2]["params"], json!({"refreshToken": false}));
}

#[test]
fn codex_gets_none_of_the_daemons_other_variables() {
    let home = Home::with_quota_home();
    let codex = home.codex(
        subscription(),
        by_limit_id(
            bucket(window(5.0, Some(300), None), Value::Null),
            json!(true),
        ),
    );

    read(&codex, &home);

    let environment = home.record("environment").unwrap();
    let names: Vec<&str> = environment
        .lines()
        .filter_map(|line| line.split_once('=').map(|(name, _)| name))
        .collect();
    // The shell adds its own working-directory and level variables.
    for name in &names {
        assert!(
            ["PATH", "HOME", "CODEX_HOME", "PWD", "OLDPWD", "SHLVL", "_"].contains(name),
            "{name} reached Codex: {names:?}"
        );
    }
    assert!(
        environment.contains(&format!("\nHOME={}\n", home.path().display()))
            || environment.starts_with(&format!("HOME={}\n", home.path().display())),
        "{environment}"
    );
}

#[test]
fn a_request_from_the_server_with_the_same_id_is_not_taken_for_an_answer() {
    let home = Home::with_quota_home();
    let mut answers = vec![json!({"id": 2, "method": "item/tool/requestUserInput", "params": {}})];
    answers.extend(signed_in_answers(
        subscription(),
        by_limit_id(
            bucket(window(5.0, Some(300), None), Value::Null),
            json!(true),
        ),
    ));
    let codex = home.codex_answering(&answers, "cat > /dev/null");

    assert_eq!(read(&codex, &home).windows().len(), 1);
}

#[test]
fn a_server_that_keeps_running_after_its_input_ends_is_stopped() {
    let home = Home::with_quota_home();
    let codex = home.codex_answering(
        &signed_in_answers(
            subscription(),
            by_limit_id(
                bucket(window(5.0, Some(300), None), Value::Null),
                json!(true),
            ),
        ),
        "exec sleep 30",
    );

    let started = std::time::Instant::now();
    read(&codex, &home);

    assert!(started.elapsed() < Duration::from_secs(10));
    let pid = home.record("pid").unwrap();
    assert!(
        !Path::new("/proc").join(pid.trim()).exists(),
        "pid {pid} still runs"
    );
}

#[test]
fn a_home_without_the_quota_sign_in_is_refused_without_running_codex() {
    let home = Home::without_quota_home();
    let codex = home.codex(subscription(), json!({}));

    let reason = refusal(&codex, &home);

    assert!(reason.contains("~/.local/share/codex-quota"), "{reason}");
    assert!(reason.contains("codex-usage.sh --login"), "{reason}");
    assert_eq!(home.record("words"), None);
}

#[test]
fn a_quota_home_without_a_subscription_sign_in_is_refused() {
    for account in [Value::Null, json!({"type": "apiKey"})] {
        let home = Home::with_quota_home();
        let codex = home.codex(account.clone(), json!({}));

        let reason = refusal(&codex, &home);

        assert!(
            reason.contains("codex-usage.sh --login"),
            "{account}: {reason}"
        );
    }
}

#[test]
fn a_refused_rate_limit_read_is_refused_without_codexs_words() {
    let home = Home::with_quota_home();
    let codex = home.codex_answering(
        &[
            json!({"id": 2, "result": {"account": subscription(), "requiresOpenaiAuth": true}}),
            json!({"id": 3, "error": {"code": -32600, "message": "private detail"}}),
        ],
        "exec sleep 30",
    );

    let reason = refusal(&codex, &home);

    assert!(reason.contains("rate limits"), "{reason}");
    assert!(!reason.contains("private"), "{reason}");
}

#[test]
fn an_answer_outside_the_protocol_is_refused_without_echoing_it() {
    for rate_limits in [
        json!({"rateLimitsByLimitId": {"other": {"private": 1}}}),
        json!({"rateLimitsByLimitId": "private"}),
        json!({"rateLimits": {"limitId": "private"}, "rateLimitsByLimitId": null}),
        json!({"rateLimits": bucket(json!({"usedPercent": "private"}), Value::Null)}),
        json!({"rateLimits": bucket(window(5.0, Some(300), None), json!({"usedPercent": 1, "resetsAt": "private"}))}),
        json!({"rateLimits": bucket(window(5.0, Some(300), None), window(6.0, Some(300), None))}),
    ] {
        let home = Home::with_quota_home();
        let codex = home.codex(subscription(), rate_limits.clone());

        let reason = refusal(&codex, &home);

        assert!(!reason.contains("private"), "{rate_limits}: {reason}");
    }
}

#[test]
fn a_server_that_stops_before_answering_is_refused() {
    let home = Home::with_quota_home();
    let codex = home.codex_answering(&[json!({"id": 1, "result": {}})], "exit 0");

    let reason = refusal(&codex, &home);

    assert!(reason.contains("stopped before it answered"), "{reason}");
}

#[test]
fn a_server_that_never_answers_is_stopped_at_its_deadline() {
    let home = Home::with_quota_home();
    let codex = home.codex_answering(&[], "exec sleep 30");

    let started = std::time::Instant::now();
    let reason = codex
        .quota_within(home.path(), Duration::from_millis(300))
        .unwrap_err()
        .reason;

    assert!(reason.contains("did not answer"), "{reason}");
    assert!(started.elapsed() < Duration::from_secs(5));
}

/// Against the installed pinned harness in an isolated home: both reads come back from its
/// app-server, and an empty quota home is no subscription sign-in. No credential is used.
#[test]
#[ignore = "needs the installed pinned harness (COCKPIT_CODEX_PROGRAM); uses only an isolated home"]
fn the_installed_harness_answers_both_reads_in_an_empty_quota_home() {
    let installed = std::env::var("COCKPIT_CODEX_PROGRAM").expect("harness path");
    let home = Home::with_quota_home();
    let quoted = |path: &str| format!("'{}'", path.replace('\'', "'\\''"));
    let isolated = quoted(&home.path().display().to_string());
    let wrapper = common::script(
        home.path(),
        "codex",
        &format!(
            "exec /usr/bin/env -i PATH=/usr/bin:/bin HOME={isolated} XDG_CONFIG_HOME={isolated}/config \
             XDG_DATA_HOME={isolated}/.local/share XDG_CACHE_HOME={isolated}/cache \
             XDG_STATE_HOME={isolated}/state NO_COLOR=1 CODEX_HOME=\"$CODEX_HOME\" {} \"$@\"",
            quoted(&installed)
        ),
    );
    let codex = Codex::new(wrapper, Some(CONFORMANCE_VERSION.to_owned()), None);

    let reason = refusal(&codex, &home);

    assert!(reason.contains("codex-usage.sh --login"), "{reason}");
    assert!(home.quota_home().join("installation_id").exists());
}
