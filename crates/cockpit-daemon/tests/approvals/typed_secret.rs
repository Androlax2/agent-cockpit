//! #1386: a secret the owner typed for a run is replaced by `[redacted]` wherever that run's
//! output goes, raw or encoded, even when a program prints it anyway. The daemon keeps only
//! keyed fingerprints of it, for as long as the run lasts.
use super::*;

/// A space, a slash and a `!` give it a URL encoding apart from the raw text.
const TYPED_SECRET: &str = "hunter 22/xyz!";

/// Asks `Enter it: ` with echo off, reads the owner's line into `$typed`, then runs `prints`.
fn reading_the_secret_then(prints: &str) -> String {
    format!(
        "printf 'Enter it: '; stty -echo; read -r typed; stty echo; printf '\\n'; \
         printf 'before '; {prints}; printf ' after\\n'"
    )
}

/// The run's last output line and its typed-secret count, as the agent and as the owner read
/// them from the stored row.
async fn typed_secret_run(asking: &Asking, prints: &str) -> [(String, u64); 2] {
    let request = asking
        .waiting_at_a_prompt(&reading_the_secret_then(prints))
        .await;
    asking.type_line(request.id, TYPED_SECRET).await;
    let to_agent = asking.ended(request.id).await.result.expect("a result");
    let to_owner = shown_to_owner(asking, request.id)
        .await
        .result
        .expect("a result");
    [to_agent, to_owner].map(|result| {
        let last = result.output.lines().last().unwrap_or_default().to_owned();
        (last, result.typed_secret_redacted)
    })
}

#[tokio::test]
async fn a_typed_secret_printed_raw_or_encoded_is_redacted_in_every_copy_of_the_output() {
    let asking = started(true).await;
    // What `[redacted]` leaves of each form: the base64 characters that also hold the bytes
    // around the secret stay.
    let forms = [
        ("raw", r#"printf '%s' "$typed""#, "before [redacted] after"),
        (
            "base64, aligned",
            r#"printf '%s' "$typed" | base64 -w0"#,
            "before [redacted]E= after",
        ),
        (
            "base64, one byte in",
            r#"printf 'x%s' "$typed" | base64 -w0"#,
            "before eG[redacted] after",
        ),
        (
            "base64, two bytes in",
            r#"printf 'xx%s' "$typed" | base64 -w0"#,
            "before eHh[redacted]Q== after",
        ),
        (
            "lower-case hex",
            r#"printf '%s' "$typed" | od -An -v -tx1 | tr -d ' \n'"#,
            "before [redacted] after",
        ),
        (
            "upper-case hex",
            r#"printf '%s' "$typed" | od -An -v -tx1 | tr -d ' \n' | tr a-f A-F"#,
            "before [redacted] after",
        ),
        (
            "URL-encoded",
            r#"printf '%s' "$typed" | sed 's/ /%20/g; s#/#%2F#g; s/!/%21/g'"#,
            "before [redacted] after",
        ),
    ];

    for (form, prints, redacted) in forms {
        for (copy, (line, count)) in ["agent", "owner"]
            .into_iter()
            .zip(typed_secret_run(&asking, prints).await)
        {
            assert_eq!(line, redacted, "{form}, as the {copy} reads it");
            assert_eq!(count, 1, "{form}, as the {copy} reads it");
        }
    }
    assert_eq!(
        files_holding(asking.home.root.path(), TYPED_SECRET.as_bytes()),
        Vec::<std::path::PathBuf>::new()
    );
}

#[tokio::test]
async fn a_typed_secret_split_across_two_output_reads_is_redacted() {
    let asking = started(true).await;

    let [(line, count), _] = typed_secret_run(
        &asking,
        r#"printf '%s' "$typed" | head -c 6; sleep 0.3; printf '%s' "$typed" | tail -c +7"#,
    )
    .await;

    assert_eq!(line, "before [redacted] after");
    assert_eq!(count, 1);
}

#[tokio::test]
async fn output_without_the_typed_secret_is_unchanged_byte_for_byte() {
    let asking = started(true).await;

    let [(line, count), _] = typed_secret_run(&asking, "printf 'hunter 22/xyz? hunter 2'").await;

    assert_eq!(line, "before hunter 22/xyz? hunter 2 after");
    assert_eq!(count, 0);
}

#[tokio::test]
async fn the_daemons_memory_holds_no_copy_of_the_typed_secret_after_the_run() {
    let asking = started(true).await;
    typed_secret_run(&asking, r#"printf '%s' "$typed""#).await;

    let daemon = asking.daemon.as_ref().expect("a daemon runs").child.id();

    assert_eq!(
        memory_regions_holding(daemon, TYPED_SECRET.as_bytes()),
        Vec::<String>::new()
    );
}

/// Every file under `dir` that holds `needle`.
fn files_holding(dir: &std::path::Path, needle: &[u8]) -> Vec<std::path::PathBuf> {
    let mut holding = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return holding;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            holding.extend(files_holding(&path, needle));
        } else if kind.is_file() && std::fs::read(&path).is_ok_and(|bytes| holds(&bytes, needle)) {
            holding.push(path);
        }
    }
    holding
}

/// The readable mappings of process `pid` (`/proc/<pid>/maps` lines) that hold `needle`:
/// the memory dump the test build allows its parent to read.
fn memory_regions_holding(pid: u32, needle: &[u8]) -> Vec<String> {
    use std::io::{Read, Seek, SeekFrom};
    let maps = std::fs::read_to_string(format!("/proc/{pid}/maps")).expect("the maps read");
    let mut memory = std::fs::File::open(format!("/proc/{pid}/mem")).expect("the memory opens");
    let mut holding = Vec::new();
    for region in maps.lines() {
        let mut fields = region.split_whitespace();
        let (Some(range), Some(permissions)) = (fields.next(), fields.next()) else {
            continue;
        };
        if !permissions.starts_with('r') || region.ends_with("[vvar]") {
            continue;
        }
        let (start, end) = range.split_once('-').expect("a range");
        let start = u64::from_str_radix(start, 16).expect("a hex start");
        let end = u64::from_str_radix(end, 16).expect("a hex end");
        let mut bytes = vec![0; usize::try_from(end - start).expect("a region fits")];
        if memory.seek(SeekFrom::Start(start)).is_err() || memory.read_exact(&mut bytes).is_err() {
            continue;
        }
        if holds(&bytes, needle) {
            holding.push(region.to_owned());
        }
    }
    holding
}

fn holds(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}
