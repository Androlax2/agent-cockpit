//! The plan windows of the account Claude Code is signed in to (#58), from the OAuth usage
//! endpoint the owner's `config/waybar/.config/waybar/scripts/claude-usage.sh` reads, with
//! its beta header. The token is Claude Code's own: read from its credentials file at each
//! request, given to `curl` on its standard input, never refreshed (a refresh from two
//! places invalidates one of them), never in an error. The hook bridge reads the windows of
//! the session's own sign-in the same way, to tell a quota stop from a transient rate limit
//! (#249).

use std::fmt;
use std::io::Read;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cockpit_provider_api::{LimitKey, ProviderError, QuotaReading, QuotaWindow};
use serde_json::Value;

use crate::sign_in::bounded_output;

pub const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
/// The `curl` on `PATH`, which asks for the plan windows unless the adapter is given another.
pub const USAGE_CLIENT: &str = "curl";
const USAGE_BETA: &str = "anthropic-beta: oauth-2025-04-20";
/// Under the owner's home, as `needs::CREDENTIAL_FILES` declares it.
const CREDENTIALS: &str = ".claude/.credentials.json";
const MAX_CREDENTIALS_BYTES: u64 = 64 * 1024;
/// How long a usage request may take, the hook bridge's included: well within Claude Code's
/// hook timeout.
pub const QUOTA_DEADLINE: Duration = Duration::from_secs(15);
const FIVE_HOURS: u32 = 5 * 3600;
const SEVEN_DAYS: u32 = 7 * 86_400;

/// The scoped weekly windows of the flat answer, with the scope each counts.
const FLAT_SCOPED: [(&str, &str); 2] = [("seven_day_opus", "Opus"), ("seven_day_sonnet", "Sonnet")];

impl crate::ClaudeCode {
    /// [`cockpit_provider_api::Provider::quota`], giving up after `deadline`.
    pub fn quota_within(
        &self,
        home: &Path,
        deadline: Duration,
    ) -> Result<QuotaReading, ProviderError> {
        let sign_in = read_sign_in(home)?;
        let body = fetch_usage(&self.usage_client, &sign_in.token, deadline)?;
        reading_of(&body, sign_in.plan)
    }
}

/// The plan windows of a sandboxed session's own sign-in (#249), read as Claude Code finds
/// it: `bound_token` first, the `CLAUDE_CODE_OAUTH_TOKEN` the session holds (a placeholder
/// the session proxy swaps for the account's credential on the usage host, #968), else the
/// credentials file under `home`, a login account's bound there (#59). `usage_client` is the
/// `curl` that asks; giving up after `deadline`.
pub fn session_quota(
    home: Option<&Path>,
    bound_token: Option<&str>,
    usage_client: &Path,
    deadline: Duration,
) -> Result<QuotaReading, ProviderError> {
    let sign_in = match (bound_token, home) {
        (Some(token), _) => SignIn {
            token: checked_token(token, "the token in CLAUDE_CODE_OAUTH_TOKEN")?,
            plan: None,
        },
        (None, Some(home)) => read_sign_in(home)?,
        (None, None) => {
            return Err(refused(
                "the session holds no CLAUDE_CODE_OAUTH_TOKEN and HOME is not set",
            ));
        }
    };
    let body = fetch_usage(usage_client, &sign_in.token, deadline)?;
    reading_of(&body, sign_in.plan)
}

fn fetch_usage(
    usage_client: &Path,
    token: &Token,
    deadline: Duration,
) -> Result<Vec<u8>, ProviderError> {
    let max_time = deadline
        .saturating_sub(Duration::from_secs(1))
        .as_secs()
        .max(1)
        .to_string();
    let header = format!("Authorization: Bearer {}\n", token.0);
    let output = bounded_output(
        usage_client,
        &[
            "--silent",
            "--proto",
            "=https",
            "--max-time",
            &max_time,
            "--header",
            "@-",
            "--header",
            USAGE_BETA,
            "--header",
            "Accept: application/json",
            "--write-out",
            "\\n%{http_code}",
            USAGE_URL,
        ],
        Some(header.as_bytes()),
        deadline,
        "usage request",
    )
    .map_err(refused)?;
    let (body, status_line) = match output.stdout.iter().rposition(|byte| *byte == b'\n') {
        Some(at) => (&output.stdout[..at], &output.stdout[at + 1..]),
        None => (&[][..], &output.stdout[..]),
    };
    let http_code = std::str::from_utf8(status_line)
        .ok()
        .and_then(|code| code.parse::<u16>().ok());
    match (output.status.code(), http_code) {
        (Some(0), Some(200)) => Ok(body.to_vec()),
        (Some(0), Some(code @ (401 | 403))) => Err(refused(format!(
            "the usage endpoint refused Claude Code's sign-in (HTTP {code}); start `claude` \
             so it signs in again"
        ))),
        (Some(0), Some(code)) => Err(refused(format!("the usage endpoint answered HTTP {code}"))),
        (Some(0), None) => Err(refused("the usage request printed no HTTP status")),
        (Some(exit), _) => Err(refused(format!(
            "the usage request failed: curl exited with {exit}{}",
            curl_failure(exit)
        ))),
        (None, _) => Err(refused("the usage request was stopped by a signal")),
    }
}

/// What a common curl exit status means. curl's own message is not repeated: it is the
/// host's text, not ours, and could carry more than the cause.
fn curl_failure(exit: i32) -> &'static str {
    match exit {
        6 => " (cannot resolve the host)",
        7 => " (cannot connect)",
        28 => " (timed out)",
        35 | 60 => " (the TLS handshake or certificate failed)",
        _ => "",
    }
}

/// Claude Code's access token; never printed.
struct Token(String);

impl fmt::Debug for Token {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Token(..)")
    }
}

struct SignIn {
    token: Token,
    plan: Option<String>,
}

fn read_sign_in(home: &Path) -> Result<SignIn, ProviderError> {
    let file = std::fs::File::open(home.join(CREDENTIALS)).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            refused(
                "Claude Code is not signed in: ~/.claude/.credentials.json is missing; run \
                 `claude` and sign in",
            )
        } else {
            refused(format!("cannot read ~/.claude/.credentials.json: {error}"))
        }
    })?;
    let mut text = Vec::new();
    file.take(MAX_CREDENTIALS_BYTES + 1)
        .read_to_end(&mut text)
        .map_err(|error| refused(format!("cannot read ~/.claude/.credentials.json: {error}")))?;
    if text.len() as u64 > MAX_CREDENTIALS_BYTES {
        return Err(refused("~/.claude/.credentials.json is over 64 KiB"));
    }
    let credentials: Value = serde_json::from_slice(&text)
        .map_err(|_| refused("~/.claude/.credentials.json is not JSON"))?;
    let oauth = &credentials["claudeAiOauth"];
    let Some(token) = oauth["accessToken"]
        .as_str()
        .filter(|token| !token.is_empty())
    else {
        return Err(refused(
            "~/.claude/.credentials.json holds no subscription sign-in (an API key has no plan \
             windows); run `claude` and sign in with a subscription",
        ));
    };
    let token = checked_token(token, "the token in ~/.claude/.credentials.json")?;
    match &oauth["expiresAt"] {
        // No expiry written: the endpoint decides, and a refusal says to sign in again.
        Value::Null => {}
        expires_at => {
            let Some(expires_at_ms) = expires_at.as_f64() else {
                return Err(refused(
                    "the `expiresAt` in ~/.claude/.credentials.json is not a number",
                ));
            };
            if expires_at_ms <= now_ms() as f64 {
                return Err(refused(
                    "Claude Code's sign-in in ~/.claude/.credentials.json expired; start \
                     `claude` so it refreshes it",
                ));
            }
        }
    }
    Ok(SignIn {
        token,
        plan: oauth["subscriptionType"].as_str().map(str::to_owned),
    })
}

/// `token` as a header line can carry it: non-empty, visible ASCII only, since anything else
/// could end the line early. `what` names it in a refusal, never its value.
fn checked_token(token: &str, what: &str) -> Result<Token, ProviderError> {
    if token.is_empty() {
        return Err(refused(format!("{what} is empty")));
    }
    if !token.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(refused(format!("{what} is malformed")));
    }
    Ok(Token(token.to_owned()))
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

/// The answer in either of its shapes: the `limits` list, or the older flat keys. A window
/// whose share used is null is left out, so a reading may have none (an account not used yet).
fn reading_of(body: &[u8], plan: Option<String>) -> Result<QuotaReading, ProviderError> {
    let usage: Value = serde_json::from_slice(body)
        .map_err(|_| refused("the usage endpoint answered something other than JSON"))?;
    let windows = match usage.get("limits") {
        Some(Value::Array(limits)) => limits
            .iter()
            .filter_map(|limit| limit_window(limit).transpose())
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => return Err(refused("the usage answer's `limits` is not a list")),
        None if usage.is_object() => flat_windows(&usage)?,
        None => return Err(refused("the usage answer is not an object")),
    };
    QuotaReading::new(plan, windows, false)
        .map_err(|error| refused(format!("the usage answer breaks a rule: {error}")))
}

fn limit_window(limit: &Value) -> Result<Option<QuotaWindow>, ProviderError> {
    let kind = limit["kind"]
        .as_str()
        .ok_or_else(|| refused("a usage limit has no kind"))?;
    // A limit scoped to a model on one surface names both, so neither alone keys it.
    let names: Vec<&str> = ["model", "surface"]
        .iter()
        .filter_map(|part| limit["scope"][part]["display_name"].as_str())
        .filter(|name| !name.is_empty())
        .collect();
    let scope = (!names.is_empty()).then(|| names.join(" / "));
    if kind == "weekly_scoped" && scope.is_none() {
        return Err(refused("a scoped usage limit names no model or surface"));
    }
    let (base, length) = match kind {
        "session" => ("five-hour".to_owned(), Some(FIVE_HOURS)),
        "weekly_all" | "weekly_scoped" => ("seven-day".to_owned(), Some(SEVEN_DAYS)),
        other => (slug(other), None),
    };
    window(
        &base,
        scope.as_deref(),
        length,
        &limit["percent"],
        &limit["resets_at"],
    )
}

/// Refuses an object with none of the flat keys: that is no usage answer.
fn flat_windows(usage: &Value) -> Result<Vec<QuotaWindow>, ProviderError> {
    let mut windows = Vec::new();
    let mut is_usage = usage.get("model_scoped").is_some();
    for (key, window_value) in usage.as_object().into_iter().flatten() {
        let (base, scope, length) = match key.as_str() {
            "five_hour" => ("five-hour".to_owned(), None, Some(FIVE_HOURS)),
            "seven_day" => ("seven-day".to_owned(), None, Some(SEVEN_DAYS)),
            "model_scoped" => continue,
            other => match FLAT_SCOPED.iter().find(|(flat, _)| *flat == other) {
                Some((_, scope)) => ("seven-day".to_owned(), Some(*scope), Some(SEVEN_DAYS)),
                None if window_value.get("utilization").is_some() => (slug(other), None, None),
                None => continue,
            },
        };
        is_usage = true;
        windows.extend(window(
            &base,
            scope,
            length,
            &window_value["utilization"],
            &window_value["resets_at"],
        )?);
    }
    for scoped in usage["model_scoped"].as_array().into_iter().flatten() {
        let scope = scoped["display_name"]
            .as_str()
            .filter(|name| !name.is_empty())
            .ok_or_else(|| refused("a model-scoped usage window has no name"))?;
        let found = window(
            "seven-day",
            Some(scope),
            Some(SEVEN_DAYS),
            &scoped["utilization"],
            &scoped["resets_at"],
        )?;
        // A model with a key of its own (`seven_day_sonnet`) may be listed here again.
        windows.extend(
            found.filter(|found| windows.iter().all(|known| known.limit() != found.limit())),
        );
    }
    if !is_usage {
        return Err(refused("the usage answer has none of its known windows"));
    }
    Ok(windows)
}

/// One window, keyed `base` or `base:<scope>`; none while the endpoint gives no share used.
fn window(
    base: &str,
    scope: Option<&str>,
    length: Option<u32>,
    used: &Value,
    resets_at: &Value,
) -> Result<Option<QuotaWindow>, ProviderError> {
    if used.is_null() {
        return Ok(None);
    }
    let used_percent = used
        .as_f64()
        .ok_or_else(|| refused(format!("the usage window `{base}` gives no number used")))?;
    let resets_at = match resets_at {
        Value::Null => None,
        Value::String(text) => Some(parse_timestamp(text).ok_or_else(|| {
            refused(format!(
                "the usage window `{base}` has an unreadable reset time"
            ))
        })?),
        _ => {
            return Err(refused(format!(
                "the usage window `{base}` has an unreadable reset time"
            )));
        }
    };
    let key = match scope.map(slug) {
        Some(scoped) if scoped.is_empty() => {
            return Err(refused(format!(
                "a usage window `{base}` names a scope with no letter or digit"
            )));
        }
        Some(scoped) => format!("{base}:{scoped}"),
        None => base.to_owned(),
    };
    let broken = |error| refused(format!("the usage answer breaks a rule: {error}"));
    let limit = LimitKey::new(key).map_err(broken)?;
    QuotaWindow::new(
        limit,
        scope.map(str::to_owned),
        length,
        used_percent,
        resets_at,
    )
    .map(Some)
    .map_err(broken)
}

/// `Opus 4.5` as `opus-4-5`: lower-case letters and digits, one dash for each run of others.
/// A letter or digit outside ASCII is written by its code point (`小` as `u5c0f`), so a
/// name in another script still has a key of its own.
fn slug(text: &str) -> String {
    let mut slug = String::new();
    let separate = |slug: &mut String| {
        if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    };
    for character in text.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
        } else if character.is_alphanumeric() {
            separate(&mut slug);
            slug.push_str(&format!("u{:x}-", u32::from(character)));
        } else {
            separate(&mut slug);
        }
    }
    slug.trim_end_matches('-').to_owned()
}

/// Seconds since the epoch of an RFC 3339 time such as `2027-01-15T10:00:00.25+02:00`, its
/// fraction dropped.
fn parse_timestamp(text: &str) -> Option<i64> {
    let (date, time) = text.split_once('T')?;
    let [year, month, day] = fields(date, '-')?;
    let zone_at = time.find(['Z', 'z', '+', '-'])?;
    let (clock, zone) = time.split_at(zone_at);
    let clock = match clock.split_once('.') {
        Some((whole, fraction))
            if !fraction.is_empty() && fraction.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            whole
        }
        Some(_) => return None,
        None => clock,
    };
    let [hour, minute, second] = fields(clock, ':')?;
    let offset = match zone {
        "Z" | "z" => 0,
        _ => {
            let sign = if zone.starts_with('-') { -1 } else { 1 };
            let digits = zone[1..].replace(':', "");
            if digits.len() != 4 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            let [hours, minutes] = [&digits[..2], &digits[2..]].map(|part| part.parse::<i64>());
            let (hours, minutes) = (hours.ok()?, minutes.ok()?);
            if hours > 23 || minutes > 59 {
                return None;
            }
            sign * (hours * 3600 + minutes * 60)
        }
    };
    let in_range = (1..=12).contains(&month)
        && (1..=days_in_month(year, month)).contains(&day)
        && hour < 24
        && minute < 60
        && second <= 60;
    in_range.then(|| {
        days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second - offset
    })
}

/// `N` unsigned numbers separated by `separator`, nothing else.
fn fields<const N: usize>(text: &str, separator: char) -> Option<[i64; N]> {
    let mut numbers = [0; N];
    let mut parts = text.split(separator);
    for number in &mut numbers {
        let part = parts.next()?;
        if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        *number = part.parse().ok()?;
    }
    parts.next().is_none().then_some(numbers)
}

fn days_in_month(year: i64, month: i64) -> i64 {
    let is_leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    match month {
        2 if is_leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's `days_from_civil`,
/// <https://howardhinnant.github.io/date_algorithms.html>).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_from_march = (month + 9) % 12;
    let day_of_year = (153 * month_from_march + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn refused(reason: impl Into<String>) -> ProviderError {
    ProviderError {
        reason: reason.into(),
    }
}
