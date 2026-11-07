//! The plan windows of Codex's quota sign-in (#58): a sign-in of its own under
//! `~/.local/share/codex-quota`, as the owner's `config/waybar/.config/waybar/scripts/codex-quota.py`
//! keeps it, never the sessions' `~/.codex` (owner's decision, 2026-11-07). Read with
//! `account/read` and `account/rateLimits/read` from `codex app-server` on its standard streams,
//! the requests and answers that script uses; the fields are those of Codex's own
//! `codex app-server generate-ts` on 0.166.0. The adapter reads nothing of the sign-in itself:
//! Codex does, in that home, and no token passes through Torlek.

use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use cockpit_provider_api::{LimitKey, ProviderError, QuotaReading, QuotaWindow};
use rustix::process::{Pid, Signal};
use serde_json::{Value, json};

pub(crate) const QUOTA_DEADLINE: Duration = Duration::from_secs(15);
/// Under the owner's home, where `codex-quota.py` signs in.
const QUOTA_HOME: &str = ".local/share/codex-quota";
const NOT_SIGNED_IN: &str = "Codex's quota sign-in in ~/.local/share/codex-quota is missing or \
                             not a subscription; sign in there with `codex-usage.sh --login`";
const MAX_OUTPUT_BYTES: usize = 64 * 1024;
const ACCOUNT_REQUEST: u64 = 2;
const RATE_LIMITS_REQUEST: u64 = 3;
const FIVE_HOURS_MINUTES: u32 = 5 * 60;
const SEVEN_DAYS_MINUTES: u32 = 7 * 24 * 60;

impl crate::Codex {
    /// [`cockpit_provider_api::Provider::quota`], giving up after `deadline`.
    pub fn quota_within(
        &self,
        home: &Path,
        deadline: Duration,
    ) -> Result<QuotaReading, ProviderError> {
        let quota_home = home.join(QUOTA_HOME);
        if !quota_home.is_dir() {
            return Err(refused(NOT_SIGNED_IN));
        }
        let answers = self.ask_app_server(&quota_home, deadline)?;
        let account = result_of(&answers.account, "Codex could not read its quota sign-in")?;
        if account["account"]["type"] != "chatgpt" {
            return Err(refused(NOT_SIGNED_IN));
        }
        let limits = result_of(&answers.rate_limits, "Codex could not read its rate limits")?;
        reading_of(&account["account"], limits)
    }

    /// Both answers, from an app-server in `quota_home` that is stopped once they arrive.
    fn ask_app_server(
        &self,
        quota_home: &Path,
        deadline: Duration,
    ) -> Result<Answers, ProviderError> {
        use std::os::fd::OwnedFd;
        use std::os::unix::net::UnixStream;

        let (mut stdout, out) = UnixStream::pair()
            .and_then(|(read, write)| {
                read.set_nonblocking(true)?;
                Ok((read, Stdio::from(OwnedFd::from(write))))
            })
            .map_err(|error| refused(format!("cannot create the app-server's pipe: {error}")))?;
        let mut child = Command::new(&self.program)
            .args(["app-server", "-c", "cli_auth_credentials_store=\"file\""])
            .env("CODEX_HOME", quota_home)
            .current_dir(quota_home)
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(out)
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| refused(format!("cannot run Codex's app-server: {error}")))?;
        let pid = Pid::from_child(&child);
        let ends_at = Instant::now() + deadline;
        let mut stdin = child.stdin.take();
        let result = (|| {
            // Four short lines fit the pipe's buffer, so this write never waits on the server,
            // which answers them in any order.
            if let Some(pipe) = stdin.as_mut() {
                pipe.write_all(&requests())
                    .map_err(|_| refused("cannot write to Codex's app-server"))?;
            }
            let mut output = Vec::new();
            let mut answers = Answers::default();
            loop {
                let has_ended = read_available(&mut stdout, &mut output)?;
                answers.take_lines(&mut output)?;
                if answers.is_complete() {
                    return Ok(answers);
                }
                if has_ended {
                    return Err(refused("Codex's app-server stopped before it answered"));
                }
                if Instant::now() >= ends_at {
                    return Err(refused(format!(
                        "Codex's app-server did not answer within {} ms",
                        deadline.as_millis()
                    )));
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        })();
        drop(stdin);
        // The server would run until its input ends; it and anything it started stop here.
        let cleanup = rustix::process::kill_process_group(pid, Signal::KILL);
        if cleanup.is_err() {
            let _ = child.kill();
        }
        child
            .wait()
            .map_err(|_| refused("cannot reap Codex's app-server"))?;
        match cleanup {
            Ok(()) | Err(rustix::io::Errno::SRCH) => {}
            Err(_) => return Err(refused("cannot stop Codex's app-server's process group")),
        }
        result
    }
}

/// `initialize`, `initialized`, then both reads, one JSON message per line.
fn requests() -> Vec<u8> {
    let messages = [
        json!({"id": 1, "method": "initialize", "params": {
            "clientInfo": {"name": "torlek_quota", "version": env!("CARGO_PKG_VERSION")}
        }}),
        json!({"method": "initialized", "params": {}}),
        json!({"id": ACCOUNT_REQUEST, "method": "account/read", "params": {"refreshToken": false}}),
        json!({"id": RATE_LIMITS_REQUEST, "method": "account/rateLimits/read", "params": {}}),
    ];
    let mut lines = Vec::new();
    for message in messages {
        lines.extend(message.to_string().into_bytes());
        lines.push(b'\n');
    }
    lines
}

#[derive(Default)]
struct Answers {
    account: Option<Value>,
    rate_limits: Option<Value>,
}

impl Answers {
    /// Takes every whole line out of `output`; notifications and other answers are skipped.
    fn take_lines(&mut self, output: &mut Vec<u8>) -> Result<(), ProviderError> {
        while let Some(end) = output.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = output.drain(..=end).collect();
            let message: Value = serde_json::from_slice(&line)
                .map_err(|_| refused("Codex's app-server wrote something other than JSON"))?;
            match message["id"].as_u64() {
                Some(ACCOUNT_REQUEST) => self.account = Some(message),
                Some(RATE_LIMITS_REQUEST) => self.rate_limits = Some(message),
                _ => {}
            }
        }
        Ok(())
    }

    fn is_complete(&self) -> bool {
        self.account.is_some() && self.rate_limits.is_some()
    }
}

/// Whether the server closed its output.
fn read_available(
    pipe: &mut impl std::io::Read,
    output: &mut Vec<u8>,
) -> Result<bool, ProviderError> {
    let mut buffer = [0; 4096];
    loop {
        match pipe.read(&mut buffer) {
            Ok(0) => return Ok(true),
            Ok(count) => {
                if output.len() + count > MAX_OUTPUT_BYTES {
                    return Err(refused("Codex's app-server wrote more than 64 KiB"));
                }
                output.extend_from_slice(&buffer[..count]);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(false),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => {
                return Err(refused(format!(
                    "cannot read Codex's app-server's output: {error}"
                )));
            }
        }
    }
}

/// The `result` of `answer`; its error's text is Codex's, so it is not repeated.
fn result_of<'answer>(
    answer: &'answer Option<Value>,
    failure: &str,
) -> Result<&'answer Value, ProviderError> {
    match answer {
        Some(message) if message["result"].is_object() => Ok(&message["result"]),
        _ => Err(refused(failure)),
    }
}

/// The `codex` bucket's windows. `ordinaryUsageAllowed: false` is Codex's word that use is
/// refused now; `null` says nothing.
fn reading_of(account: &Value, limits: &Value) -> Result<QuotaReading, ProviderError> {
    let bucket = match &limits["rateLimitsByLimitId"] {
        Value::Object(buckets) => buckets
            .get("codex")
            .ok_or_else(|| refused("Codex's rate limits have no `codex` bucket"))?,
        Value::Null => &limits["rateLimits"],
        _ => return Err(refused("Codex's rate limits are not keyed by limit")),
    };
    if !bucket.is_object() || !matches!(bucket["limitId"].as_str(), None | Some("codex")) {
        return Err(refused("Codex's rate limits have no `codex` bucket"));
    }
    let mut windows = Vec::new();
    for slot in ["primary", "secondary"] {
        if !bucket[slot].is_null() {
            windows.push(window(slot, &bucket[slot])?);
        }
    }
    let plan = account["planType"].as_str().map(str::to_owned);
    let is_blocked = limits["ordinaryUsageAllowed"] == false;
    QuotaReading::new(plan, windows, is_blocked).map_err(broken)
}

/// A window keyed by its length, so it keeps its name whichever slot Codex puts it in:
/// `five-hour`, `seven-day`, `window-<minutes>-minutes`, or the slot's name without a length.
fn window(slot: &str, value: &Value) -> Result<QuotaWindow, ProviderError> {
    let unreadable = || refused(format!("Codex's `{slot}` rate-limit window is unreadable"));
    let used_percent = value["usedPercent"].as_f64().ok_or_else(unreadable)?;
    let minutes = match &value["windowDurationMins"] {
        Value::Null => None,
        minutes => Some(
            minutes
                .as_u64()
                .and_then(|minutes| u32::try_from(minutes).ok())
                .filter(|minutes| *minutes > 0)
                .ok_or_else(unreadable)?,
        ),
    };
    let resets_at = match &value["resetsAt"] {
        Value::Null => None,
        resets_at => Some(
            resets_at
                .as_i64()
                .filter(|at| *at >= 0)
                .ok_or_else(unreadable)?,
        ),
    };
    let key = match minutes {
        Some(FIVE_HOURS_MINUTES) => "five-hour".to_owned(),
        Some(SEVEN_DAYS_MINUTES) => "seven-day".to_owned(),
        Some(minutes) => format!("window-{minutes}-minutes"),
        None => slot.to_owned(),
    };
    let length = minutes
        .map(|minutes| minutes.checked_mul(60).ok_or_else(unreadable))
        .transpose()?;
    QuotaWindow::new(
        LimitKey::new(key).map_err(broken)?,
        None,
        length,
        used_percent,
        resets_at,
    )
    .map_err(broken)
}

fn broken(error: impl std::fmt::Display) -> ProviderError {
    refused(format!("Codex's rate limits break a rule: {error}"))
}

fn refused(reason: impl Into<String>) -> ProviderError {
    ProviderError {
        reason: reason.into(),
    }
}
