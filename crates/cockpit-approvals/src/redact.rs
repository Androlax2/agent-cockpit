//! What a run's output must not carry back to the agent: terminal control sequences, and
//! secrets by their shape or by the name in front of them. The patterns are the current
//! approval terminal's (`apps/crates/cockpit-core/src/approval.rs`), carried over unchanged.

use std::sync::LazyLock;

use regex::{Captures, Regex};

const REDACTED: &str = "[redacted]";

/// Escape sequences (CSI, OSC, keypad modes) and carriage returns: a terminal interprets
/// them, an agent reading the text would only be misled by them.
static TERMINAL_CONTROLS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b[=>]|\r")
        .expect("the pattern compiles")
});

/// Tokens and keys recognisable by their shape, and private key blocks: replaced whole.
static SECRET_SHAPES: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"/pair/[0-9a-fA-F]{32}\b",
        r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY(?: BLOCK)?-----[\s\S]*?(?:-----END [A-Z0-9 ]*PRIVATE KEY(?: BLOCK)?-----|\z)",
        r"\b(?:sk|rk)_(?:live|test)_[A-Za-z0-9]{16,}",
        r"\bwhsec_[A-Za-z0-9+/=]{16,}",
        r"\bya29\.[A-Za-z0-9_-]{20,}",
        r"https://hooks\.slack\.com/services/[A-Za-z0-9/]+",
        r"\bgh[pousr]_[A-Za-z0-9]{30,}\b",
        r"\bgithub_pat_[A-Za-z0-9_]{20,}\b",
        r"\bsk-ant-[A-Za-z0-9_-]{20,}",
        r"\bsk-(?:proj-)?[A-Za-z0-9_-]{20,}",
        r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b",
        r"\bxox[abprs]-[A-Za-z0-9-]{10,}",
        r"\bAIza[0-9A-Za-z_-]{35}\b",
        r"\bglpat-[A-Za-z0-9_-]{20,}",
        r"\bnpm_[A-Za-z0-9]{36}\b",
        r"\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}",
    ]
    .iter()
    .map(|pattern| Regex::new(pattern).expect("the pattern compiles"))
    .collect()
});

/// The value after a name that says it is secret, after an Authorization scheme, or the
/// password in `scheme://user:password@host`. Group 1 is kept, group 2 (when present) too;
/// what lies between becomes `[redacted]`, so the agent still sees what was there.
static SECRET_VALUES: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"(?im)(Or type this code in the Home Screen app within [0-9]+ minutes:[ \t]*)[0-9]{3}[ \t]*[0-9]{3}\b",
        r#"(?i)((?:authorization|proxy-authorization)\s*:\s*(?:bearer|basic|token)\s+)[^\s"']+"#,
        r"(?i)((?:set-)?cookie\s*:\s*)[^\n]+",
        // JSON: a quoted secret value may hold spaces ("token": "…", Docker's "auth": "…").
        r#"(?i)("[a-z0-9_.-]*(?:password|passwd|passphrase|secret|token|api[_-]?key|access[_-]?key|private[_-]?key|credentials?|auth)[a-z0-9_.-]*"\s*:\s*")[^"]+(")"#,
        r#"(?i)(\b[a-z0-9_.-]*(?:password|passwd|passphrase|secret|token|api[_-]?key|access[_-]?key|private[_-]?key|credentials?)[a-z0-9_.-]*\b["']?\s*[:=]\s*["']?)[^\s"']+"#,
        // .netrc and .pgpass style: the keyword, a space, the value.
        r"(?im)(\b(?:password|passwd)\s+)[^\s=:]\S*",
        r"(?i)([a-z][a-z0-9+.-]*://[^\s:/@]+:)[^\s@/]+(@)",
    ]
    .iter()
    .map(|pattern| Regex::new(pattern).expect("the pattern compiles"))
    .collect()
});

/// `text` without terminal control sequences.
pub fn strip_terminal_controls(text: &str) -> String {
    TERMINAL_CONTROLS.replace_all(text, "").into_owned()
}

/// `text` with every secret-looking value replaced by `[redacted]`, and how many were.
pub fn redact_secrets(text: &str) -> (String, usize) {
    let mut count = 0;
    let mut redacted = text.to_owned();
    for shape in SECRET_SHAPES.iter() {
        count += shape.find_iter(&redacted).count();
        redacted = shape.replace_all(&redacted, REDACTED).into_owned();
    }
    for value in SECRET_VALUES.iter() {
        redacted = value
            .replace_all(&redacted, |captures: &Captures| {
                // A value a shape above already replaced is not counted twice.
                if captures[0].ends_with(REDACTED) {
                    return captures[0].to_owned();
                }
                count += 1;
                format!(
                    "{}{REDACTED}{}",
                    &captures[1],
                    captures.get(2).map_or("", |end| end.as_str())
                )
            })
            .into_owned();
    }
    (redacted, count)
}
