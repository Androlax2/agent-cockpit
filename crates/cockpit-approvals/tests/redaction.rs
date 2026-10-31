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
