//! Plan windows through the public provider contract, against a scratch home and a fake
//! `curl`: no test reaches the network or the owner's own sign-in.
mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use cockpit_claude::ClaudeCode;
use cockpit_provider_api::{Provider, QuotaReading};

const TOKEN: &str = "sk-ant-oat01-test-token";
/// 2100-01-01, in milliseconds as Claude Code writes it.
const LATER_MS: i64 = 4_102_444_800_000;
/// 2027-01-15 08:00 UTC.
const NOW: i64 = 1_800_000_000;
const HOUR: i64 = 3600;

struct Home {
    dir: tempfile::TempDir,
}

impl Home {
    fn signed_in() -> Home {
        let home = Home::empty();
        home.write_credentials(&format!(
            r#"{{"claudeAiOauth":{{"accessToken":"{TOKEN}","refreshToken":"sk-ant-ort01-refresh","expiresAt":{LATER_MS},"subscriptionType":"max"}}}}"#
        ));
        home
    }

    fn empty() -> Home {
        Home {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn write_credentials(&self, text: &str) {
        let folder = self.dir.path().join(".claude");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join(".credentials.json"), text).unwrap();
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    /// A `curl` that records its words and standard input beside the home, then prints
    /// `body` and the status line `--write-out` asks for.
    fn curl(&self, body: &str, http_code: &str) -> ClaudeCode {
        let body_file = self.path().join("body");
        std::fs::write(&body_file, body).unwrap();
        self.curl_running(&format!(
            "cat '{}'; printf '\\n{http_code}'",
            body_file.display()
        ))
    }

    fn curl_running(&self, script: &str) -> ClaudeCode {
        let record = self.path().display().to_string();
        let program = common::script(
            self.path(),
            "curl",
            &format!(
                "printf '%s\\n' \"$@\" > '{record}/curl-words'\ncat > '{record}/curl-input'\n{script}"
            ),
        );
        ClaudeCode::new(PathBuf::from("/usr/bin/claude"), None, None).with_usage_client(program)
    }

    fn curl_words(&self) -> Option<String> {
        std::fs::read_to_string(self.path().join("curl-words")).ok()
    }

    fn curl_input(&self) -> String {
        std::fs::read_to_string(self.path().join("curl-input")).unwrap()
    }
}

/// A saved answer of the usage endpoint, under `tests/fixtures/oauth-usage`.
fn usage(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/oauth-usage")
        .join(format!("{name}.json"));
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn read(claude: &ClaudeCode, home: &Home) -> QuotaReading {
    claude.quota(home.path()).unwrap()
}

fn refusal(claude: &ClaudeCode, home: &Home) -> String {
    claude.quota(home.path()).unwrap_err().reason
}

/// limit, scope, length in seconds, share used, reset.
type Window<'reading> = (
    &'reading str,
    Option<&'reading str>,
    Option<u32>,
    f64,
    Option<i64>,
);

fn windows(reading: &QuotaReading) -> Vec<Window<'_>> {
    reading
        .windows()
        .iter()
        .map(|window| {
            (
                window.limit().as_str(),
                window.scope(),
                window.length_seconds(),
                window.used_percent(),
                window.resets_at(),
            )
        })
        .collect()
}

#[test]
fn the_limits_list_gives_each_window_with_its_scope_and_reset() {
    let home = Home::signed_in();
    let claude = home.curl(&usage("limits"), "200");

    let reading = read(&claude, &home);

    assert_eq!(
        windows(&reading),
        [
            ("five-hour", None, Some(18_000), 42.5, Some(NOW + 2 * HOUR)),
            (
                "seven-day",
                None,
                Some(604_800),
                81.0,
                Some(NOW + 122 * HOUR)
            ),
            (
                "seven-day:opus-4-5",
                Some("Opus 4.5"),
                Some(604_800),
                12.0,
                Some(NOW + 122 * HOUR)
            ),
        ]
    );
    assert!(!reading.is_blocked());
}

#[test]
fn the_older_flat_answer_gives_the_same_limit_names() {
    let home = Home::signed_in();
    let claude = home.curl(&usage("flat"), "200");

    let reading = read(&claude, &home);

    let mut found = windows(&reading);
    found.sort_by(|left, right| left.0.cmp(right.0));
    assert_eq!(
        found,
        [
            ("five-hour", None, Some(18_000), 7.0, Some(NOW + 2 * HOUR)),
            (
                "seven-day",
                None,
                Some(604_800),
                33.0,
                Some(NOW + 122 * HOUR)
            ),
            ("seven-day:fable", Some("Fable"), Some(604_800), 50.0, None),
            (
                "seven-day:sonnet",
                Some("Sonnet"),
                Some(604_800),
                4.0,
                Some(NOW + 122 * HOUR)
            ),
        ]
    );
}

#[test]
fn a_model_listed_under_its_own_key_and_again_as_model_scoped_is_one_window() {
    let home = Home::signed_in();
    let claude = home.curl(
        r#"{"seven_day_sonnet":{"utilization":4.0,"resets_at":null},
            "model_scoped":[{"display_name":"Sonnet","utilization":4.0,"resets_at":null}]}"#,
        "200",
    );

    let reading = read(&claude, &home);

    assert_eq!(
        windows(&reading),
        [("seven-day:sonnet", Some("Sonnet"), Some(604_800), 4.0, None)]
    );
}

#[test]
fn a_limit_scoped_to_a_model_on_a_surface_is_keyed_by_both() {
    let home = Home::signed_in();
    let claude = home.curl(
        r#"{"limits":[
            {"kind":"weekly_scoped","percent":1,"resets_at":null,
             "scope":{"model":{"display_name":"Opus"},"surface":{"display_name":"Code"}}},
            {"kind":"weekly_scoped","percent":2,"resets_at":null,
             "scope":{"model":{"display_name":"Opus"},"surface":{"display_name":"Chat"}}}]}"#,
        "200",
    );

    let reading = read(&claude, &home);

    let keys: Vec<&str> = windows(&reading).iter().map(|window| window.0).collect();
    assert_eq!(keys, ["seven-day:opus-code", "seven-day:opus-chat"]);
}

#[test]
fn a_scope_named_outside_ascii_keeps_a_key_of_its_own() {
    let home = Home::signed_in();
    let claude = home.curl(
        r#"{"model_scoped":[{"display_name":"小","utilization":3,"resets_at":null}]}"#,
        "200",
    );

    let reading = read(&claude, &home);

    assert_eq!(
        windows(&reading)[0].0,
        "seven-day:u5c0f",
        "{:?}",
        windows(&reading)
    );
}

#[test]
fn an_answer_whose_windows_have_no_share_yet_is_a_reading_without_windows() {
    let home = Home::signed_in();
    let claude = home.curl(
        r#"{"five_hour":{"utilization":null,"resets_at":null},"seven_day":null}"#,
        "200",
    );

    let reading = read(&claude, &home);

    assert!(reading.windows().is_empty(), "{:?}", windows(&reading));
}

#[test]
fn a_sign_in_without_an_expiry_is_left_to_the_endpoint() {
    let home = Home::empty();
    home.write_credentials(&format!(
        r#"{{"claudeAiOauth":{{"accessToken":"{TOKEN}"}}}}"#
    ));
    let claude = home.curl(&usage("limits"), "200");

    let reading = read(&claude, &home);

    assert_eq!(reading.windows().len(), 3);
}

#[test]
fn an_expiry_that_is_no_number_is_refused_as_such_without_asking() {
    let home = Home::empty();
    home.write_credentials(&format!(
        r#"{{"claudeAiOauth":{{"accessToken":"{TOKEN}","expiresAt":"soon"}}}}"#
    ));
    let claude = home.curl("{}", "200");

    let reason = refusal(&claude, &home);

    assert!(reason.contains("`expiresAt`"), "{reason}");
    assert!(!reason.contains("expired"), "{reason}");
    assert_eq!(home.curl_words(), None);
}

#[test]
fn the_plan_is_the_subscription_claude_code_signed_in_to() {
    let home = Home::signed_in();
    let claude = home.curl(&usage("limits"), "200");

    assert_eq!(read(&claude, &home).plan(), Some("max"));
}

#[test]
fn the_token_goes_to_curl_on_its_input_never_in_its_words() {
    let home = Home::signed_in();
    let claude = home.curl(&usage("limits"), "200");

    read(&claude, &home);

    let words = home.curl_words().unwrap();
    assert!(!words.contains(TOKEN), "{words}");
    assert!(!words.contains("refresh"), "{words}");
    assert_eq!(
        home.curl_input(),
        format!("Authorization: Bearer {TOKEN}\n")
    );
    let words: Vec<&str> = words.lines().collect();
    assert_eq!(
        words.last(),
        Some(&"https://api.anthropic.com/api/oauth/usage")
    );
    for expected in ["@-", "anthropic-beta: oauth-2025-04-20", "=https"] {
        assert!(words.contains(&expected), "{words:?} lacks {expected}");
    }
}

#[test]
fn an_expired_token_is_refused_without_asking_and_never_refreshed() {
    let home = Home::empty();
    home.write_credentials(&format!(
        r#"{{"claudeAiOauth":{{"accessToken":"{TOKEN}","expiresAt":1000}}}}"#
    ));
    let claude = home.curl("{}", "200");

    let reason = refusal(&claude, &home);

    assert!(reason.contains("expired"), "{reason}");
    assert!(reason.contains("`claude`"), "{reason}");
    assert_eq!(home.curl_words(), None);
}

#[test]
fn a_home_without_a_subscription_sign_in_is_refused_without_asking() {
    for credentials in [
        None,
        Some("not json"),
        Some(r#"{"claudeAiOauth":{}}"#),
        Some(r#"{"claudeAiOauth":{"accessToken":"line\nbreak","expiresAt":4102444800000}}"#),
    ] {
        let home = Home::empty();
        if let Some(text) = credentials {
            home.write_credentials(text);
        }
        let claude = home.curl("{}", "200");

        let reason = refusal(&claude, &home);

        assert!(
            reason.contains(".credentials.json"),
            "{credentials:?}: {reason}"
        );
        assert!(!reason.contains("line"), "{reason}");
        assert_eq!(home.curl_words(), None, "{credentials:?}");
    }
}

#[test]
fn a_refused_sign_in_says_to_sign_in_again() {
    let home = Home::signed_in();
    let claude = home.curl(r#"{"error":"private detail"}"#, "401");

    let reason = refusal(&claude, &home);

    assert!(reason.contains("HTTP 401"), "{reason}");
    assert!(reason.contains("`claude`"), "{reason}");
    assert!(!reason.contains("private"), "{reason}");
}

#[test]
fn a_limited_or_failed_request_is_refused_without_its_output() {
    for (script, expected) in [
        ("printf '{\"error\":\"private\"}\\n429'", "HTTP 429"),
        ("printf 'private\\n500'", "HTTP 500"),
        (
            "echo 'curl: (6) private host' >&2; printf '\\n000'; exit 6",
            "curl exited with 6 (cannot resolve the host)",
        ),
    ] {
        let home = Home::signed_in();
        let claude = home.curl_running(script);

        let reason = refusal(&claude, &home);

        assert!(reason.contains(expected), "{reason}");
        assert!(!reason.contains("private"), "{reason}");
        assert!(!reason.contains(TOKEN), "{reason}");
    }
}

#[test]
fn an_answer_in_neither_shape_is_refused_without_echoing_it() {
    for body in [
        "private, not json",
        r#"["private"]"#,
        r#"{"limits":{"private":1}}"#,
        r#"{"private":"nothing"}"#,
        r#"{"limits":[{"kind":"session","percent":"private"}]}"#,
        r#"{"limits":[{"kind":"session","percent":5,"resets_at":"private"}]}"#,
        r#"{"limits":[{"kind":"session","percent":5,"resets_at":"2027-02-31T00:00:00Z"}]}"#,
        r#"{"limits":[{"kind":"weekly_scoped","percent":5,"resets_at":null}]}"#,
    ] {
        let home = Home::signed_in();
        let claude = home.curl(body, "200");

        let reason = refusal(&claude, &home);

        assert!(!reason.contains("private"), "{body}: {reason}");
    }
}

#[test]
fn a_request_that_hangs_is_stopped_at_its_deadline() {
    let home = Home::signed_in();
    let claude = home.curl_running("sleep 30");

    let started = std::time::Instant::now();
    let reason = claude
        .quota_within(home.path(), Duration::from_millis(300))
        .unwrap_err()
        .reason;

    assert!(reason.contains("did not answer"), "{reason}");
    assert!(started.elapsed() < Duration::from_secs(5));
}
