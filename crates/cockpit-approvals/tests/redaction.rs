//! What a run's output must not carry back to the agent.

use cockpit_approvals::{present, redact_secrets, strip_terminal_controls};

#[test]
fn tokens_are_redacted_by_their_shape() {
    let (text, count) = redact_secrets(
        "GH=ghp_0123456789abcdefghijABCDEFGHIJ012345 and sk-ant-api03-abcdefghijklmnopqrstuvwxyz",
    );
    assert_eq!(count, 2, "{text}");
    assert_eq!(text, "GH=[redacted] and [redacted]");
}

#[test]
fn a_private_key_block_is_redacted_whole() {
    let (text, count) = redact_secrets(
        "before\n-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXk\n-----END OPENSSH PRIVATE KEY-----\nafter",
    );
    assert_eq!((text.as_str(), count), ("before\n[redacted]\nafter", 1));
}

#[test]
fn named_secrets_keep_their_name() {
    let (text, count) =
        redact_secrets("DB_PASSWORD=hunter2hunter\napi_key: \"abcd1234efgh\"\nport=5432");
    assert_eq!(count, 2, "{text}");
    assert_eq!(
        text,
        "DB_PASSWORD=[redacted]\napi_key: \"[redacted]\"\nport=5432"
    );
}

#[test]
fn sudos_password_prompt_is_not_a_secret() {
    let prompt = "[sudo] password for theo: ";

    assert_eq!(redact_secrets(prompt), (prompt.to_owned(), 0));
}

#[test]
fn a_netrc_password_is_still_redacted() {
    let (text, count) = redact_secrets("machine example.com login theo password hunter22");

    assert_eq!(
        (text.as_str(), count),
        ("machine example.com login theo password [redacted]", 1)
    );
}

#[test]
fn credentials_in_urls_and_headers_are_redacted() {
    let (text, count) =
        redact_secrets("https://owner:s3cret@example.com/x\nAuthorization: Bearer abc.def.ghi");
    assert_eq!(count, 2, "{text}");
    assert_eq!(
        text,
        "https://owner:[redacted]@example.com/x\nAuthorization: Bearer [redacted]"
    );
}

#[test]
fn terminal_controls_are_stripped() {
    assert_eq!(
        strip_terminal_controls("\u{1b}[1;31mred\u{1b}[0m\r\n\u{1b}]0;title\u{7}done"),
        "red\ndone"
    );
}

#[test]
fn every_control_that_could_hide_text_is_stripped() {
    let cases = [
        (
            "backspace",
            "secret\u{8}\u{8}\u{8}\u{8}\u{8}\u{8}shown",
            "secretshown",
        ),
        (
            "device control string",
            "a\u{1b}Pq#0;2;0;0;0\u{1b}\\b",
            "ab",
        ),
        (
            "application program command",
            "a\u{1b}_payload\u{1b}\\b",
            "ab",
        ),
        ("privacy message", "a\u{1b}^payload\u{1b}\\b", "ab"),
        ("terminal reset", "before\u{1b}cafter", "beforeafter"),
        ("charset switch", "a\u{1b}(0b\u{1b}(Bc", "abc"),
        ("C1 control", "a\u{9b}b\u{85}c", "abc"),
        ("bell and form feed", "a\u{7}b\u{c}c", "abc"),
        ("unterminated title", "a\u{1b}]0;title", "a"),
    ];
    for (name, raw, stripped) in cases {
        assert_eq!(strip_terminal_controls(raw), stripped, "{name}");
    }
}

#[test]
fn newlines_and_tabs_are_kept() {
    assert_eq!(strip_terminal_controls("a\tb\nc"), "a\tb\nc");
}

#[test]
fn a_key_block_whose_begin_line_was_cut_off_is_redacted() {
    let kept = b"MIIEvQIBADANBgkqhkiG9w0BAQEFAASC\nMIIEpAIBAAKCAQEAy8Dbv8prpJ\n\
-----END RSA PRIVATE KEY-----\nafter the key\n";

    let presented = present(kept, 10_000, 10);

    assert!(!presented.shown.contains("MIIE"), "{}", presented.shown);
    assert_eq!(presented.shown, "[redacted]\nafter the key");
    assert_eq!(presented.redacted, 1);
}

#[test]
fn a_whole_key_block_in_cut_output_is_redacted_as_one() {
    let kept = b"cut line\nbefore\n-----BEGIN PRIVATE KEY-----\nMIIE\n\
-----END PRIVATE KEY-----\nafter\n";

    let presented = present(kept, 10_000, 10);

    assert_eq!(presented.shown, "before\n[redacted]\nafter");
    assert_eq!(presented.redacted, 1);
}

#[test]
fn a_value_a_shape_already_replaced_is_counted_once() {
    let (redacted, count) =
        redact_secrets(r#"{"token": "ghp_abcdefghijklmnopqrstuvwxyz0123456789"}"#);

    assert_eq!(redacted, r#"{"token": "[redacted]"}"#);
    assert_eq!(count, 1);
}

#[test]
fn the_answer_quotes_the_last_lines_and_counts_the_rest() {
    let output: String = (1..=5).map(|line| format!("line {line}\r\n")).collect();
    let presented = present(output.as_bytes(), output.len() as u64, 2);
    assert_eq!(presented.shown, "line 4\nline 5");
    assert_eq!(presented.omitted_lines, 3);
    assert_eq!(presented.truncated_bytes, 0);
}

#[test]
fn output_cut_at_the_front_starts_at_its_first_whole_line() {
    let kept = b"rtial line\nwhole line\n";
    let presented = present(kept, 100, 10);
    assert_eq!(presented.shown, "whole line");
    assert_eq!(presented.truncated_bytes, 100 - 22 + 11);
}

#[test]
fn secrets_in_the_output_are_redacted_and_counted() {
    let output = b"token=abcdefgh12345678\nok\n";
    let presented = present(output, output.len() as u64, 10);
    assert_eq!(presented.shown, "token=[redacted]\nok");
    assert_eq!(presented.redacted, 1);
}

#[test]
fn an_answer_is_bounded_to_16_kib_and_counts_the_bytes_it_leaves_out() {
    let line = format!("{}\n", "x".repeat(10 * 1024 - 1));
    let output = line.repeat(120);
    let presented = present(output.as_bytes(), output.len() as u64, 120);
    assert!(
        presented.shown.len() <= 16 * 1024,
        "{}",
        presented.shown.len()
    );
    assert_eq!(
        presented.truncated_bytes + presented.shown.len() as u64 + 1,
        output.len() as u64
    );
}

#[test]
fn a_name_that_only_contains_a_secret_word_keeps_what_follows_it() {
    let cases = [
        "skip greenfield-857-forge-credentials-decision: no target/",
        "src/token_store.rs:12: error",
        "token_store.rs:12:5: warning: unused import",
        "token-refresh.rs: 2 tests",
        "secret-scan: 3 files",
        "cat: credentials.json: No such file or directory",
    ];
    for line in cases {
        let presented = present(line.as_bytes(), line.len() as u64, 10);
        assert_eq!(
            (presented.shown.as_str(), presented.redacted),
            (line, 0),
            "{line}"
        );
    }
}

#[test]
fn a_secret_after_its_name_is_still_redacted() {
    let cases = [
        ("GH_TOKEN=ghp_x", "GH_TOKEN=[redacted]"),
        ("api-key: abc", "api-key: [redacted]"),
        ("X-Auth-Token: abc", "X-Auth-Token: [redacted]"),
        ("client_secret = \"abc\"", "client_secret = \"[redacted]\""),
        (
            "\"credentials\": \"abc\"",
            "\"credentials\": \"[redacted]\"",
        ),
        (
            "export AWS_SECRET_ACCESS_KEY=abc",
            "export AWS_SECRET_ACCESS_KEY=[redacted]",
        ),
        (
            "aws.secret_access_key = abc",
            "aws.secret_access_key = [redacted]",
        ),
        ("--token=abc", "--token=[redacted]"),
        ("--secret-key=abc", "--secret-key=[redacted]"),
        (
            "password-confirmation=abc",
            "password-confirmation=[redacted]",
        ),
        ("jwt.secret.key=abc", "jwt.secret.key=[redacted]"),
        ("Secret-Key: abc", "Secret-Key: [redacted]"),
        ("secret_key_base: abc", "secret_key_base: [redacted]"),
    ];
    for (line, redacted) in cases {
        let presented = present(line.as_bytes(), line.len() as u64, 10);
        assert_eq!(
            (presented.shown.as_str(), presented.redacted),
            (redacted, 1),
            "{line}"
        );
    }
}

#[test]
fn a_token_after_a_name_that_only_contains_a_secret_word_is_redacted_by_its_shape() {
    let cases = [
        (
            "secret-scan: ghp_0123456789abcdefghijABCDEFGHIJ012345",
            "secret-scan: [redacted]",
        ),
        (
            "src/token_store.rs:12: ghp_0123456789abcdefghijABCDEFGHIJ012345",
            "src/token_store.rs:12: [redacted]",
        ),
    ];
    for (line, redacted) in cases {
        let presented = present(line.as_bytes(), line.len() as u64, 10);
        assert_eq!(
            (presented.shown.as_str(), presented.redacted),
            (redacted, 1),
            "{line}"
        );
    }
}
