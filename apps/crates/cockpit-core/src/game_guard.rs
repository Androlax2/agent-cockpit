//! Reads the admission status the host's game guard publishes (`publish_gate` in
//! config/bin/.local/bin/cockpit-game-guard), with the checks agent-build's `read_gate` makes. The
//! daemon lets agent turns continue during a game only while that guard holds new heavy jobs: any
//! doubt about the file means it does not.

use serde_json::Value;
use std::ffi::{CStr, OsStr};
use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

const GATE_VERSION: i64 = 1;
const GATE_MAX_BYTES: u64 = 4096;
const GATE_FRESH_S: f64 = 30.0;
const GATE_AHEAD_S: f64 = 5.0;

/// Where the guard writes it: under the account's home from the password database, never $HOME,
/// as the guard and agent-build resolve it.
pub fn gate_path() -> Result<PathBuf, String> {
    Ok(account_home()?.join(".cache/agent-shims/heavy-gate.json"))
}

/// Ok when the guard, alive this boot, holds new heavy jobs: for an open game, or by Theo's
/// `cockpit-game-guard hold`, which takes precedence over it in the status; otherwise why not.
pub fn holds_heavy_jobs_for_game(
    path: &Path,
    now_s: f64,
    current_boot: Option<&str>,
) -> Result<(), String> {
    let gate = read_gate(path)?;
    if gate.get("version").and_then(Value::as_i64) != Some(GATE_VERSION) {
        return Err("the game guard status has an unknown format".into());
    }
    if gate.get("writer").and_then(Value::as_str) != Some("guard") {
        return Err("the game guard status was not written by the guard".into());
    }
    let Some(current_boot) = current_boot else {
        return Err("this boot's id is unreadable".into());
    };
    if gate.get("boot_id").and_then(Value::as_str) != Some(current_boot) {
        return Err("the game guard status is from an earlier boot".into());
    }
    let Some(updated_at) = gate.get("updated_at").and_then(Value::as_f64) else {
        return Err("the game guard status has no valid time".into());
    };
    let age = now_s - updated_at;
    if !(-GATE_AHEAD_S..=GATE_FRESH_S).contains(&age) {
        return Err(format!(
            "the game guard status is stale (last written {} s ago)",
            age as i64
        ));
    }
    if gate
        .get("manual_release_until")
        .and_then(Value::as_f64)
        .is_some_and(|until| now_s < until)
    {
        return Err("Theo released heavy jobs (cockpit-game-guard release)".into());
    }
    match gate.get("state").and_then(Value::as_str) {
        Some("gaming" | "held") => Ok(()),
        Some(state) => Err(format!("the game guard admission is {state}")),
        None => Err("the game guard status has no state".into()),
    }
}

fn read_gate(path: &Path) -> Result<serde_json::Map<String, Value>, String> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)
        .map_err(|error| format!("the game guard status is unreadable: {error}"))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("the game guard status is unreadable: {error}"))?;
    if !metadata.is_file() {
        return Err("the game guard status is not a regular file".into());
    }
    // SAFETY: getuid takes no pointer and cannot fail.
    if metadata.uid() != unsafe { libc::getuid() } {
        return Err("the game guard status belongs to another user".into());
    }
    let mut content = Vec::new();
    file.take(GATE_MAX_BYTES + 1)
        .read_to_end(&mut content)
        .map_err(|error| format!("the game guard status is unreadable: {error}"))?;
    if content.len() as u64 > GATE_MAX_BYTES {
        return Err("the game guard status is too large".into());
    }
    match serde_json::from_slice(&content) {
        Ok(Value::Object(gate)) => Ok(gate),
        _ => Err("the game guard status is not a JSON object".into()),
    }
}

fn account_home() -> Result<PathBuf, String> {
    // SAFETY: getuid takes no pointer and cannot fail.
    let uid = unsafe { libc::getuid() };
    // SAFETY: passwd is plain C data (integers and pointers); all zeroes is a valid value that
    // getpwuid_r overwrites before we read it.
    let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
    let mut found: *mut libc::passwd = std::ptr::null_mut();
    let mut buffer = vec![0 as libc::c_char; 16 * 1024];
    // SAFETY: every pointer is to a live local; getpwuid_r writes at most buffer.len() bytes into
    // buffer and points entry's strings into it, so they stay valid while buffer lives.
    let status = unsafe {
        libc::getpwuid_r(
            uid,
            &mut entry,
            buffer.as_mut_ptr(),
            buffer.len(),
            &mut found,
        )
    };
    if status != 0 || found.is_null() || entry.pw_dir.is_null() {
        return Err(format!("no password entry for uid {uid}"));
    }
    // SAFETY: on success pw_dir is a NUL-terminated string inside buffer, which outlives this
    // borrow; the bytes are copied into the PathBuf before buffer is dropped.
    let home = PathBuf::from(OsStr::from_bytes(
        unsafe { CStr::from_ptr(entry.pw_dir) }.to_bytes(),
    ));
    if !home.is_absolute() {
        return Err(format!(
            "the home of uid {uid} is not an absolute path: {home:?}"
        ));
    }
    Ok(home)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::fs;

    const NOW: f64 = 1_793_459_201.0;
    const BOOT: &str = "1a912d37-3e34-4722-a686-6958f0bdfbdd";

    fn gaming_gate() -> Value {
        json!({
            "version": 1, "state": "gaming", "reason": "a game is open", "since": NOW - 60.0,
            "updated_at": NOW - 2.0, "sequence": 7, "boot_id": BOOT, "writer": "guard",
            "guard_pid": 4865, "memory": null, "manual_release_until": null,
        })
    }

    fn written(name: &str, content: &[u8]) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("cockpit-game-guard-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("heavy-gate.json");
        fs::write(&path, content).unwrap();
        path
    }

    fn gate_with(name: &str, change: impl FnOnce(&mut Value)) -> PathBuf {
        let mut gate = gaming_gate();
        change(&mut gate);
        written(name, gate.to_string().as_bytes())
    }

    #[test]
    fn a_fresh_gaming_status_from_the_guard_this_boot_holds_heavy_jobs() {
        let path = gate_with("fresh", |_| {});
        assert_eq!(holds_heavy_jobs_for_game(&path, NOW, Some(BOOT)), Ok(()));
    }

    #[test]
    fn any_doubt_about_the_status_means_heavy_jobs_are_not_held() {
        for (name, field, value) in [
            ("clear", "state", json!("clear")),
            ("off", "state", json!("off")),
            ("memory", "state", json!("memory")),
            ("version", "version", json!(2)),
            ("manual-writer", "writer", json!("manual release")),
            ("other-boot", "boot_id", json!("another-boot")),
            ("stale", "updated_at", json!(NOW - 31.0)),
            ("future", "updated_at", json!(NOW + 6.0)),
            ("time-text", "updated_at", json!("now")),
            ("released", "manual_release_until", json!(NOW + 600.0)),
        ] {
            let path = gate_with(name, |gate| gate[field] = value);
            assert!(
                holds_heavy_jobs_for_game(&path, NOW, Some(BOOT)).is_err(),
                "{name}"
            );
        }
    }

    #[test]
    fn an_expired_manual_release_no_longer_counts() {
        let path = gate_with("expired-release", |gate| {
            gate["manual_release_until"] = json!(NOW - 1.0)
        });
        assert_eq!(holds_heavy_jobs_for_game(&path, NOW, Some(BOOT)), Ok(()));
    }

    #[test]
    fn an_unknown_current_boot_means_heavy_jobs_are_not_held() {
        let path = gate_with("no-boot", |_| {});
        assert!(holds_heavy_jobs_for_game(&path, NOW, None).is_err());
    }

    #[test]
    fn theos_hold_from_the_guard_also_holds_heavy_jobs() {
        let path = gate_with("held", |gate| gate["state"] = json!("held"));
        assert_eq!(holds_heavy_jobs_for_game(&path, NOW, Some(BOOT)), Ok(()));
    }

    #[test]
    fn a_missing_status_means_heavy_jobs_are_not_held() {
        let path = written("missing", b"").with_file_name("absent.json");
        assert!(holds_heavy_jobs_for_game(&path, NOW, Some(BOOT)).is_err());
    }

    #[test]
    fn an_unparsable_or_oversized_status_means_heavy_jobs_are_not_held() {
        let mut padded = gaming_gate();
        padded["reason"] = json!("x".repeat(4096));
        for (name, content) in [
            ("invalid", b"{not json".to_vec()),
            ("not-object", br#"["gaming"]"#.to_vec()),
            ("oversized", padded.to_string().into_bytes()),
        ] {
            let path = written(name, &content);
            assert!(
                holds_heavy_jobs_for_game(&path, NOW, Some(BOOT)).is_err(),
                "{name}"
            );
        }
    }

    #[test]
    fn a_symbolic_link_to_a_gaming_status_is_not_trusted() {
        let target = gate_with("link", |_| {});
        let link = target.with_file_name("link.json");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(holds_heavy_jobs_for_game(&link, NOW, Some(BOOT)).is_err());
    }

    #[test]
    fn a_directory_is_not_a_status() {
        let path = written("directory", b"").with_file_name("as-directory");
        fs::create_dir_all(&path).unwrap();
        assert!(holds_heavy_jobs_for_game(&path, NOW, Some(BOOT)).is_err());
    }
}
