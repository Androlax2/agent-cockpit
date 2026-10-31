//! What a run's output must not carry back to the agent: terminal control sequences, and
//! secrets by their shape or by the name in front of them. The patterns are the current
//! approval terminal's (`apps/crates/cockpit-core/src/approval.rs`), carried over unchanged.

use std::borrow::Cow;
use std::sync::LazyLock;

use regex::{Captures, Regex};

const REDACTED: &str = "[redacted]";

/// Escape sequences (CSI, OSC, DCS, SOS, PM, APC, charset and single-character ones), C1
/// controls and every C0 control but newline and tab: a terminal interprets them, an agent
/// reading the text would only be misled by them (a backspace or a reset can hide what came
/// before).
static TERMINAL_CONTROLS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"\x1b\[[0-?]*[ -/]*[@-~]",
        r"|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)?",
        r"|\x1b[PX^_][\s\S]*?(?:\x1b\\|\z)",
        r"|\x1b[ -/]+[0-~]",
        r"|\x1b[0-~]",
        r"|[\x00-\x08\x0b-\x1f\x7f\x{80}-\x{9f}]",
    ))
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

/// The end line of a private key block.
static KEY_BLOCK_END: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"-----END [A-Z0-9 ]*PRIVATE KEY(?: BLOCK)?-----").expect("the pattern compiles")
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

/// [`redact_secrets`] for the end of a longer output: a private key block whose BEGIN line
/// was cut off is redacted from the start of `text` to its END line.
pub fn redact_secrets_after_cut(text: &str) -> (String, usize) {
    let Some(end) = KEY_BLOCK_END.find(text) else {
        return redact_secrets(text);
    };
    if text[..end.start()].contains("-----BEGIN ") {
        return redact_secrets(text);
    }
    let (rest, count) = redact_secrets(&text[end.end()..]);
    (format!("{REDACTED}{rest}"), count + 1)
}

/// `text` with every secret-looking value replaced by `[redacted]`, and how many were.
pub fn redact_secrets(text: &str) -> (String, usize) {
    let mut count = 0;
    let mut redacted = text.to_owned();
    for shape in SECRET_SHAPES.iter() {
        let replaced = shape.replace_all(&redacted, |_: &Captures| {
            count += 1;
            REDACTED
        });
        if let Cow::Owned(replaced) = replaced {
            redacted = replaced;
        }
    }
    for value in SECRET_VALUES.iter() {
        redacted = value
            .replace_all(&redacted, |captures: &Captures| {
                // A value a shape above already replaced is not counted twice.
                let value_end = captures.get(2).map_or(captures[0].len(), |end| {
                    end.start() - captures.get(0).expect("the whole match").start()
                });
                if captures[0][captures[1].len()..value_end] == *REDACTED {
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
