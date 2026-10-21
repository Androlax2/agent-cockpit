mod common;
#[path = "common/codex_quota.rs"]
mod codex_quota;

use common::sandbox::*;
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Stdio;
use std::time::{Duration, Instant};

#[test]
fn dedicated_account_quota_is_normalized_without_exposing_account_data() {
    let sandbox = codex_quota::sandbox();
    let output = codex_quota::client(&sandbox, &[]);
    assert!(output.status.success(), "{}", stderr(&output));
    let quota: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(quota, json!({"rate_limit":{
        "primary_window":{"used_percent":28,"limit_window_seconds":18000,"reset_at":4102444800i64},
        "secondary_window":{"used_percent":86,"limit_window_seconds":604800,"reset_at":4102444800i64}}}));
    assert!(!stdout(&output).contains("private@example.test"));
    assert!(stderr(&output).is_empty());
    assert_eq!(fs::metadata(sandbox.home.join(".local/share/codex-quota")).unwrap().permissions().mode() & 0o777, 0o700);
}

#[test]
fn explicit_login_waits_for_its_own_completion() {
    let sandbox = codex_quota::sandbox();
    let output = codex_quota::client(&sandbox, &["--login"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains("https://auth.openai.com/test-login"));
}

#[test]
fn unsuccessful_login_does_not_expose_server_errors() {
    let sandbox = codex_quota::sandbox();
    fs::write(sandbox.home.join("login-fails"), "").unwrap();
    let output = codex_quota::client(&sandbox, &["--login"]);
    assert!(!output.status.success());
    assert!(!stderr(&output).contains("private-server-secret"));
    assert!(stderr(&output).contains("sign-in failed"));
}

#[test]
fn non_subscription_accounts_require_a_dedicated_sign_in() {
    for account in [Value::Null, json!({"type":"apiKey"})] {
        let sandbox = codex_quota::sandbox();
        fs::write(sandbox.home.join("account-response.json"), json!({"account":account,"requiresOpenaiAuth":true}).to_string()).unwrap();
        let output = codex_quota::client(&sandbox, &[]);
        assert!(!output.status.success());
        assert!(stderr(&output).contains("--login"));
        assert!(stdout(&output).is_empty());
    }
}

#[test]
fn missing_vendor_binary_has_an_actionable_error() {
    let sandbox = codex_quota::sandbox();
    fs::remove_file(sandbox.home.join(".local/npm/bin/codex")).unwrap();
    let output = codex_quota::client(&sandbox, &[]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("Codex CLI"));
}

#[test]
fn quota_selects_only_the_codex_bucket_and_preserves_null_windows() {
    let sandbox = codex_quota::sandbox();
    fs::write(sandbox.home.join("quota-response.json"), json!({
        "rateLimits":{"limitId":"other", "primary":{"usedPercent":99}},
        "rateLimitsByLimitId":{"codex":{"limitId":"codex","primary":null,
            "secondary":{"usedPercent":42,"windowDurationMins":10080,"resetsAt":4102444800i64}},
            "other":{"primary":{"usedPercent":99}}}
    }).to_string()).unwrap();
    let output = codex_quota::client(&sandbox, &[]);
    assert!(output.status.success(), "{}", stderr(&output));
    let quota: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(quota["rate_limit"]["primary_window"].is_null());
    assert_eq!(quota["rate_limit"]["secondary_window"]["used_percent"], 42);
}

#[test]
fn malformed_or_unrelated_quota_is_not_presented_as_available() {
    for quota in [
        json!({"rateLimits": {"primary":null,"secondary":null}}),
        json!({"rateLimits": {"limitId":"other","primary":{"usedPercent":0,"windowDurationMins":300,"resetsAt":1}}}),
        json!({"rateLimitsByLimitId":{"other":{}}, "rateLimits":{"primary":{"usedPercent":0,"windowDurationMins":300,"resetsAt":1}}}),
        json!({"rateLimits": {"primary":{"usedPercent":101,"windowDurationMins":300,"resetsAt":1}}}),
        json!({"rateLimits": {"primary":{"usedPercent":false,"windowDurationMins":300,"resetsAt":1}}}),
        json!({"rateLimits": {"primary":{"usedPercent":0,"windowDurationMins":0,"resetsAt":1}}}),
        json!({"rateLimits": {"primary":{"usedPercent":0,"windowDurationMins":300,"resetsAt":"tomorrow"}}}),
    ] {
        let sandbox = codex_quota::sandbox();
        fs::write(sandbox.home.join("quota-response.json"), quota.to_string()).unwrap();
        let output = codex_quota::client(&sandbox, &[]);
        assert!(!output.status.success(), "accepted {quota}");
        assert!(stdout(&output).is_empty());
        assert!(stderr(&output).contains("Codex quota"));
    }
}

#[test]
fn broken_protocol_responses_fail_without_leaking_subprocess_output() {
    for body in [
        "import sys; print('private-server-secret', file=sys.stderr); raise SystemExit(2)",
        "print('private-server-secret', flush=True)",
        "print('[]', flush=True)",
        "print('[' * 2000 + '0' + ']' * 2000, flush=True)",
        "print('x' * (1024 * 1024 + 1), flush=True)",
        "print('{\"id\":99,\"result\":{}}', flush=True)",
        "print('{\"id\":1,\"error\":{\"message\":\"private-server-secret\"}}', flush=True)",
        "print('{\"id\":1,\"method\":\"command/exec\",\"params\":{}}', flush=True)",
    ] {
        let sandbox = codex_quota::sandbox();
        codex_quota::vendor(&sandbox, &format!("import sys\nsys.stdin.readline()\n{body}"));
        let output = codex_quota::client(&sandbox, &[]);
        assert!(!output.status.success(), "accepted {body}");
        assert!(stdout(&output).is_empty());
        assert!(!stderr(&output).contains("private-server-secret"));
        assert!(!stderr(&output).contains("Traceback"));
        assert!(stderr(&output).contains("Codex quota"));
    }
}

#[test]
fn stalled_server_is_killed_and_reaped_when_the_deadline_expires() {
    let sandbox = codex_quota::sandbox();
    codex_quota::vendor(&sandbox, r#"
import os, signal, time
from pathlib import Path
Path.home().joinpath('vendor.pid.tmp').write_text(str(os.getpid()))
Path.home().joinpath('vendor.pid.tmp').rename(Path.home().joinpath('vendor.pid'))
signal.signal(signal.SIGTERM, signal.SIG_IGN)
while True:
    time.sleep(1)
"#);
    let helper = waybar_scripts().join("codex-quota.py");
    // Accelerate only the test process's timer, leaving production deadlines intact.
    let driver = r#"
import runpy, signal, sys
set_timer = signal.setitimer
signal.setitimer = lambda kind, seconds: set_timer(kind, min(seconds, 0.2))
sys.argv = [sys.argv[1]]
runpy.run_path(sys.argv[0], run_name='__main__')
"#;
    let output = sandbox.run("python3", &["-c", driver, helper.to_str().unwrap()], &[]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("timed out"));
    let pid = fs::read_to_string(sandbox.home.join("vendor.pid")).unwrap();
    assert!(!std::path::Path::new("/proc").join(pid).exists(), "vendor process survived timeout");
}

#[test]
fn terminating_the_helper_reaps_its_stalled_server() {
    let sandbox = codex_quota::sandbox();
    codex_quota::vendor(&sandbox, r#"
import os, time
from pathlib import Path
Path.home().joinpath('vendor.pid.tmp').write_text(str(os.getpid()))
Path.home().joinpath('vendor.pid.tmp').rename(Path.home().joinpath('vendor.pid'))
while True:
    time.sleep(1)
"#);
    let helper = waybar_scripts().join("codex-quota.py");
    let child = sandbox.command("python3", &[]).arg(helper).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    let pid_path = sandbox.home.join("vendor.pid");
    while !pid_path.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let vendor_pid: i32 = fs::read_to_string(pid_path).unwrap().parse().unwrap();
    unsafe { libc::kill(child.id() as i32, libc::SIGTERM); }
    let output = finish(child, Duration::from_secs(3));
    let vendor_survived = std::path::Path::new("/proc").join(vendor_pid.to_string()).exists();
    if vendor_survived {
        unsafe { libc::kill(vendor_pid, libc::SIGKILL); }
    }
    assert!(!vendor_survived, "vendor process survived helper termination");
    assert!(!output.status.success());
    assert!(stderr(&output).contains("cancelled"));
}

#[test]
fn empty_data_home_uses_the_default_private_directory() {
    let mut sandbox = codex_quota::sandbox();
    sandbox.env.insert("XDG_DATA_HOME".into(), String::new());
    let output = sandbox.command("python3", &[]).arg(waybar_scripts().join("codex-quota.py"))
        .current_dir(&sandbox.root).output().unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(sandbox.home.join(".local/share/codex-quota").is_dir());
}
