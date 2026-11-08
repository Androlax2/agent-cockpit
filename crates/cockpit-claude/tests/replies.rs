//! Model replies through the public provider contract, against scratch homes holding
//! transcripts shaped like Claude Code's: no test reads the owner's own transcripts.
mod common;

use std::io::Write;
use std::os::unix::fs::symlink;

use cockpit_provider_api::{ModelReply, Provider, ReplyScan, ReplyTokens, SourceCursor};

const TRANSCRIPT: &str = ".claude/projects/-work-app/9f1c.jsonl";

fn assistant_line(message_id: &str, request_id: &str, usage: &str) -> String {
    format!(
        r#"{{"type":"assistant","timestamp":"2026-11-02T10:00:00.000Z","cwd":"/work/app","sessionId":"9f1c","requestId":"{request_id}","message":{{"id":"{message_id}","model":"claude-opus-5-5","role":"assistant","usage":{usage}}}}}"#
    )
}

const PLAIN_USAGE: &str = r#"{"input_tokens":10,"output_tokens":20,"cache_read_input_tokens":300,"cache_creation_input_tokens":40}"#;

struct Home {
    dir: tempfile::TempDir,
}

impl Home {
    fn new() -> Home {
        Home {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn append(&self, relative: &str, text: &str) {
        let path = self.dir.path().join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        file.write_all(text.as_bytes()).unwrap();
    }

    fn append_lines(&self, relative: &str, lines: &[String]) {
        for line in lines {
            self.append(relative, &format!("{line}\n"));
        }
    }

    fn scan(&self, cursors: &[SourceCursor]) -> ReplyScan {
        common::claude()
            .scan_replies(self.dir.path(), cursors)
            .unwrap()
    }
}

fn keys(scan: &ReplyScan) -> Vec<&str> {
    scan.replies.iter().map(ModelReply::key).collect()
}

#[test]
fn a_reply_carries_its_model_time_folder_and_every_token_count() {
    let home = Home::new();
    home.append_lines(TRANSCRIPT, &[assistant_line("msg_1", "req_1", PLAIN_USAGE)]);

    let scan = home.scan(&[]);

    let reply = &scan.replies[0];
    assert_eq!(reply.key(), "msg_1:req_1");
    assert_eq!(reply.model(), "claude-opus-5-5");
    assert_eq!(reply.at(), 1_793_613_600);
    assert_eq!(reply.working_dir(), Some("/work/app"));
    assert_eq!(
        reply.tokens(),
        ReplyTokens {
            input: 10,
            output: 20,
            cache_read: 300,
            cache_write_5m: 40,
            cache_write_1h: 0,
            is_fast: false,
        }
    );
    assert!(scan.problems.is_empty(), "{:?}", scan.problems);
}

#[test]
fn cache_writes_are_split_by_lifetime_when_the_transcript_says_so() {
    let home = Home::new();
    let usage = r#"{"input_tokens":1,"output_tokens":2,"cache_creation_input_tokens":100,"cache_creation":{"ephemeral_5m_input_tokens":30,"ephemeral_1h_input_tokens":70}}"#;
    home.append_lines(TRANSCRIPT, &[assistant_line("msg_1", "req_1", usage)]);

    let tokens = home.scan(&[]).replies[0].tokens();

    assert_eq!((tokens.cache_write_5m, tokens.cache_write_1h), (30, 70));
}

#[test]
fn a_cache_write_without_a_lifetime_split_counts_as_five_minutes() {
    let home = Home::new();
    home.append_lines(TRANSCRIPT, &[assistant_line("msg_1", "req_1", PLAIN_USAGE)]);

    let tokens = home.scan(&[]).replies[0].tokens();

    assert_eq!((tokens.cache_write_5m, tokens.cache_write_1h), (40, 0));
}

#[test]
fn a_fast_mode_reply_says_so() {
    let home = Home::new();
    let usage = r#"{"input_tokens":1,"output_tokens":2,"speed":"fast"}"#;
    home.append_lines(TRANSCRIPT, &[assistant_line("msg_1", "req_1", usage)]);

    assert!(home.scan(&[]).replies[0].tokens().is_fast);
}

#[test]
fn lines_that_cost_nothing_are_not_replies() {
    let home = Home::new();
    let synthetic =
        assistant_line("msg_2", "req_2", PLAIN_USAGE).replace("claude-opus-5-5", "<synthetic>");
    home.append_lines(
        TRANSCRIPT,
        &[
            r#"{"type":"user","timestamp":"2026-11-02T10:00:00.000Z","message":{"role":"user","content":"hello"}}"#
                .to_owned(),
            synthetic,
            assistant_line("msg_3", "req_3", r#"{"input_tokens":0,"output_tokens":0}"#),
            "not json but mentions \"usage\"".to_owned(),
            assistant_line("msg_4", "req_4", PLAIN_USAGE),
        ],
    );

    assert_eq!(keys(&home.scan(&[])), ["msg_4:req_4"]);
}

#[test]
fn a_line_without_ids_is_named_by_its_place_in_its_file() {
    let home = Home::new();
    home.append_lines(
        TRANSCRIPT,
        &[
            assistant_line("", "", PLAIN_USAGE),
            assistant_line("", "", PLAIN_USAGE),
        ],
    );

    let scan = home.scan(&[]);

    let [first, second] = keys(&scan)[..] else {
        panic!("two replies expected: {:?}", keys(&scan));
    };
    assert_ne!(first, second);
    assert!(first.starts_with(&format!("line:{TRANSCRIPT}:")), "{first}");
}

#[test]
fn a_second_scan_reads_only_what_was_appended_after_the_first() {
    let home = Home::new();
    home.append_lines(TRANSCRIPT, &[assistant_line("msg_1", "req_1", PLAIN_USAGE)]);
    let first = home.scan(&[]);

    assert!(home.scan(&first.cursors).replies.is_empty());
    assert!(home.scan(&first.cursors).cursors.is_empty());
    home.append_lines(TRANSCRIPT, &[assistant_line("msg_2", "req_2", PLAIN_USAGE)]);
    let second = home.scan(&first.cursors);

    assert_eq!(keys(&second), ["msg_2:req_2"]);
}

#[test]
fn a_line_still_being_written_is_counted_by_the_scan_after_it_ends() {
    let home = Home::new();
    let line = assistant_line("msg_1", "req_1", PLAIN_USAGE);
    let (written, unwritten) = line.split_at(line.len() / 2);
    home.append(TRANSCRIPT, written);

    let early = home.scan(&[]);
    home.append(TRANSCRIPT, &format!("{unwritten}\n"));
    let late = home.scan(&early.cursors);

    assert!(early.replies.is_empty());
    assert!(early.cursors.is_empty());
    assert_eq!(keys(&late), ["msg_1:req_1"]);
}

#[test]
fn a_transcript_replaced_by_a_shorter_one_is_read_again_from_its_start() {
    let home = Home::new();
    home.append_lines(
        TRANSCRIPT,
        &[
            assistant_line("msg_1", "req_1", PLAIN_USAGE),
            assistant_line("msg_2", "req_2", PLAIN_USAGE),
        ],
    );
    let first = home.scan(&[]);
    std::fs::remove_file(home.dir.path().join(TRANSCRIPT)).unwrap();
    home.append_lines(TRANSCRIPT, &[assistant_line("msg_3", "req_3", PLAIN_USAGE)]);

    let second = home.scan(&first.cursors);

    assert_eq!(keys(&second), ["msg_3:req_3"]);
}

#[test]
fn subagent_transcripts_are_scanned_too() {
    let home = Home::new();
    let subagent = ".claude/projects/-work-app/9f1c/subagents/agent-a1.jsonl";
    home.append_lines(subagent, &[assistant_line("msg_9", "req_9", PLAIN_USAGE)]);

    let scan = home.scan(&[]);

    assert_eq!(keys(&scan), ["msg_9:req_9"]);
    assert_eq!(scan.cursors[0].source(), subagent);
}

#[test]
fn a_home_without_transcripts_has_no_replies() {
    let home = Home::new();

    let scan = home.scan(&[]);

    assert_eq!(scan, ReplyScan::default());
}

#[test]
fn a_claude_folder_that_is_a_link_is_not_followed() {
    let outside = Home::new();
    outside.append_lines(TRANSCRIPT, &[assistant_line("msg_1", "req_1", PLAIN_USAGE)]);
    let home = Home::new();
    symlink(
        outside.dir.path().join(".claude"),
        home.dir.path().join(".claude"),
    )
    .unwrap();

    assert!(home.scan(&[]).replies.is_empty());
}

#[test]
fn a_transcript_that_is_a_link_is_not_followed() {
    let outside = Home::new();
    outside.append_lines(
        "other.jsonl",
        &[assistant_line("msg_1", "req_1", PLAIN_USAGE)],
    );
    let home = Home::new();
    let folder = home.dir.path().join(".claude/projects/-work-app");
    std::fs::create_dir_all(&folder).unwrap();
    symlink(
        outside.dir.path().join("other.jsonl"),
        folder.join("link.jsonl"),
    )
    .unwrap();
    symlink(outside.dir.path(), folder.join("linked-folder")).unwrap();

    assert!(home.scan(&[]).replies.is_empty());
}

#[test]
fn a_transcript_that_cannot_be_read_is_a_problem_the_scan_names() {
    let home = Home::new();
    home.append_lines(TRANSCRIPT, &[assistant_line("msg_1", "req_1", PLAIN_USAGE)]);
    let unreadable = ".claude/projects/-work-app/locked.jsonl";
    home.append_lines(unreadable, &[assistant_line("msg_2", "req_2", PLAIN_USAGE)]);
    let locked = home.dir.path().join(unreadable);
    std::fs::set_permissions(&locked, std::os::unix::fs::PermissionsExt::from_mode(0o000)).unwrap();
    if std::fs::File::open(&locked).is_ok() {
        // Running as root: permissions do not stop the read, so there is no problem to name.
        return;
    }

    let scan = home.scan(&[]);

    assert_eq!(keys(&scan), ["msg_1:req_1"]);
    assert_eq!(scan.problems.len(), 1, "{:?}", scan.problems);
    assert!(scan.problems[0].contains(unreadable), "{:?}", scan.problems);
}

#[test]
fn a_cache_split_that_falls_short_of_the_total_does_not_drop_tokens() {
    let home = Home::new();
    let usage = r#"{"input_tokens":1,"output_tokens":2,"cache_creation_input_tokens":5000,"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":0}}"#;
    home.append_lines(TRANSCRIPT, &[assistant_line("msg_1", "req_1", usage)]);

    let tokens = home.scan(&[]).replies[0].tokens();

    assert_eq!((tokens.cache_write_5m, tokens.cache_write_1h), (5000, 0));
}

#[test]
fn a_line_without_ids_under_a_very_long_path_is_still_a_reply() {
    let home = Home::new();
    let long_folder = "x".repeat(240);
    let transcript = format!(".claude/projects/{long_folder}/9f1c.jsonl");
    home.append_lines(&transcript, &[assistant_line("", "", PLAIN_USAGE)]);

    let scan = home.scan(&[]);

    assert_eq!(scan.replies.len(), 1);
    assert!(scan.replies[0].key().len() <= 256);
}

#[test]
fn an_impossible_date_is_not_a_reply_time() {
    let home = Home::new();
    let line = assistant_line("msg_1", "req_1", PLAIN_USAGE)
        .replace("2026-11-02T10:00:00.000Z", "2026-02-31T10:00:00.000Z");
    home.append_lines(TRANSCRIPT, &[line]);

    assert!(home.scan(&[]).replies.is_empty());
}
