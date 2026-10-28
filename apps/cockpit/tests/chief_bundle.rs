use cockpit::chief::bundle::{FormQuestion, InstructionFile, instruction_files, stop_bundle};
use cockpit::teams::ErrorCode;
use cockpit::todo::Item;
use rusqlite::Connection;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
mod common;

const SESSION: &str = "ses_stopped0000000000000000";
const TOKEN: &str = "ghp_0123456789abcdefghijABCDEFGHIJ012345";

struct Database {
    path: PathBuf,
    connection: Connection,
    next_seq: i64,
    _sandbox: common::sandbox::Sandbox,
}

impl Database {
    fn new() -> Self {
        let sandbox = common::sandbox::Sandbox::new();
        let path = sandbox.home.join("opencode.db");
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch("create table session_message (id text primary key, session_id text, type text, seq integer, data text);").unwrap();
        Self {
            path,
            connection,
            next_seq: 1,
            _sandbox: sandbox,
        }
    }

    fn add(&mut self, session: &str, role: &str, data: Value) {
        self.connection
            .execute(
                "insert into session_message values (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    format!("m{}", self.next_seq),
                    session,
                    role,
                    self.next_seq,
                    data.to_string()
                ],
            )
            .unwrap();
        self.next_seq += 1;
    }

    fn user(&mut self, text: &str) {
        self.add(SESSION, "user", json!({"text": text}));
    }

    fn assistant(&mut self, text: &str) {
        self.add(
            SESSION,
            "assistant",
            json!({"content": [{"type": "text", "text": text}]}),
        );
    }
}

fn todo_item(text: &str, notes: &str) -> Item {
    Item(
        json!({"id": "t1", "text": text, "notes": notes, "status": "doing"})
            .as_object()
            .unwrap()
            .clone(),
    )
}

fn bundle_of(database: &Path) -> cockpit::chief::bundle::StopBundle {
    stop_bundle(
        database,
        SESSION,
        Some(&todo_item("Fix login", "Keep the old API")),
        Vec::new(),
    )
    .unwrap()
}

#[test]
fn a_bundle_keeps_conversation_text_and_drops_tool_outputs() {
    let mut database = Database::new();
    database.user("Fix login");
    database.add(SESSION, "assistant", json!({"content": [
        {"type": "reasoning", "text": "private reasoning"},
        {"type": "tool", "name": "bash", "state": {"status": "completed", "input": {"command": "cat secrets"}, "output": "TOOL OUTPUT"}},
        {"type": "text", "text": "I looked at the code."},
    ]}));
    database.add(
        SESSION,
        "synthetic",
        json!({"text": "Team message arrived"}),
    );
    database.add(SESSION, "attachment", json!({"text": "ATTACHMENT BODY"}));
    database.add("ses_other", "user", json!({"text": "Other session secret"}));
    database.assistant("Which option do you want?");
    let bundle = bundle_of(&database.path);
    let roles: Vec<_> = bundle
        .recent
        .iter()
        .map(|message| (message.role.as_str(), message.text.as_str()))
        .collect();
    assert_eq!(
        roles,
        [
            ("user", "Fix login"),
            ("assistant", "I looked at the code."),
            ("synthetic", "Team message arrived"),
            ("assistant", "Which option do you want?")
        ]
    );
    assert_eq!(bundle.last_assistant, "Which option do you want?");
    assert_eq!(bundle.task_text, "Fix login");
    assert_eq!(bundle.task_notes, "Keep the old API");
    assert!(bundle.question.is_none());
    let everything = serde_json::to_string(&bundle).unwrap();
    for hidden in [
        "private reasoning",
        "TOOL OUTPUT",
        "cat secrets",
        "ATTACHMENT BODY",
        "Other session secret",
    ] {
        assert!(
            !everything.contains(hidden),
            "{hidden} leaked: {everything}"
        );
    }
}

#[test]
fn a_bundle_masks_tokens_and_escapes_hiding_characters() {
    let mut database = Database::new();
    database.user(&format!("Use GITHUB_TOKEN={TOKEN} please"));
    database.assistant("Done\u{202e}gnp.exe");
    database.add(SESSION, "assistant", json!({"content": [{"type": "tool", "name": "question", "state": {"status": "running", "input": {"questions": [
        {"header": format!("Token {TOKEN}"), "question": "Pick\u{202e}", "options": [{"label": format!("Use {TOKEN}"), "description": "d"}]}]}}}]}));
    let bundle = stop_bundle(
        &database.path,
        SESSION,
        Some(&todo_item(
            &format!("Fix with GITHUB_TOKEN={TOKEN}"),
            "note\u{200b}s",
        )),
        Vec::new(),
    )
    .unwrap();
    let everything = serde_json::to_string(&bundle).unwrap();
    assert!(!everything.contains("ghp_0123"), "{everything}");
    assert!(everything.contains("[redacted]"));
    assert!(
        !everything.contains('\u{202e}') && !everything.contains('\u{200b}'),
        "{everything}"
    );
    assert_eq!(bundle.last_assistant, "Done\\u202egnp.exe");
    assert_eq!(bundle.task_notes, "note\\u200bs");
    let question = bundle.question.unwrap();
    assert_eq!(question.question, "Pick\\u202e");
}

#[test]
fn a_bundle_includes_the_open_question_form() {
    let mut database = Database::new();
    database.user("Fix login");
    database.add(SESSION, "assistant", json!({"content": [
        {"type": "text", "text": "I have a question."},
        {"type": "tool", "name": "question", "state": {"status": "running", "input": {"questions": [
            {"header": "Approach", "question": "Which approach?", "multiple": false,
             "options": [{"label": "Small", "description": "minimal"}, {"label": "Large", "description": "full"}]}]}}},
    ]}));
    assert_eq!(
        bundle_of(&database.path).question,
        Some(FormQuestion {
            header: "Approach".into(),
            question: "Which approach?".into(),
            options: vec!["Small".into(), "Large".into()],
            question_count: 1
        })
    );
}

fn running_question(questions: Value) -> Value {
    json!({"content": [{"type": "tool", "name": "question", "state": {"status": "running", "input": {"questions": questions}}}]})
}

#[test]
fn a_question_superseded_by_a_newer_message_is_not_open() {
    let questions = json!([{"header": "Approach", "question": "Which approach?", "options": [{"label": "Small"}]}]);
    for newer in ["user", "assistant", "synthetic"] {
        let mut database = Database::new();
        database.user("Fix login");
        database.add(SESSION, "assistant", running_question(questions.clone()));
        assert!(
            bundle_of(&database.path).question.is_some(),
            "the form is open before {newer} follows"
        );
        let newer_data = if newer == "assistant" {
            json!({"content": [{"type": "text", "text": "Never mind."}]})
        } else {
            json!({"text": "Never mind."})
        };
        database.add(SESSION, newer, newer_data);
        assert!(
            bundle_of(&database.path).question.is_none(),
            "a {newer} message supersedes the running tool"
        );
    }
}

#[test]
fn a_form_with_several_questions_is_flagged() {
    let mut database = Database::new();
    database.add(
        SESSION,
        "assistant",
        running_question(json!([
        {"header": "One", "question": "First?", "options": [{"label": "A"}]},
        {"header": "Two", "question": "Second?", "options": [{"label": "B"}]}])),
    );
    let question = bundle_of(&database.path).question.unwrap();
    assert_eq!(
        (question.question_count, question.header.as_str()),
        (2, "One")
    );
}

#[test]
fn an_oversized_form_and_task_stay_inside_the_bundle_budget() {
    let mut database = Database::new();
    let options: Vec<Value> = (0..50)
        .map(|index| json!({"label": format!("{index}{}", "o".repeat(5_000))}))
        .collect();
    database.user(&"u".repeat(5_000));
    database.assistant(&"a".repeat(5_000));
    database.add(
        SESSION,
        "assistant",
        running_question(json!([
        {"header": "h".repeat(5_000), "question": "q".repeat(20_000), "options": options}])),
    );
    let bundle = stop_bundle(
        &database.path,
        SESSION,
        Some(&todo_item(&"t".repeat(50_000), &"n".repeat(50_000))),
        Vec::new(),
    )
    .unwrap();
    let question = bundle.question.as_ref().unwrap();
    assert!(question.header.chars().count() <= 200);
    assert!(question.question.chars().count() <= 1_000);
    assert!(question.options.len() <= 10);
    assert!(
        question
            .options
            .iter()
            .all(|option| option.chars().count() <= 200)
    );
    assert!(serde_json::to_string(&bundle).unwrap().len() <= 16_000);
}

#[test]
fn escape_heavy_text_cannot_push_the_serialised_bundle_over_its_budget() {
    let mut database = Database::new();
    let hidden = "\u{202e}\"\\".repeat(2_000);
    for _ in 0..25 {
        database.user(&hidden);
        database.assistant(&hidden);
    }
    database.add(SESSION, "assistant", running_question(json!([
        {"header": hidden, "question": hidden, "options": (0..20).map(|_| json!({"label": hidden})).collect::<Vec<_>>()}])));
    let bundle = stop_bundle(
        &database.path,
        SESSION,
        Some(&todo_item(&hidden, &hidden)),
        Vec::new(),
    )
    .unwrap();
    assert!(serde_json::to_string(&bundle).unwrap().len() <= 16_000);
    assert!(!bundle.recent.is_empty());
}

#[test]
fn a_huge_tool_output_does_not_hide_the_conversation_text() {
    let mut database = Database::new();
    database.user("Fix login");
    database.add(SESSION, "assistant", json!({"content": [
        {"type": "tool", "name": "bash", "state": {"status": "completed", "output": "x".repeat(4_000_000)}},
        {"type": "text", "text": "Output was long."}]}));
    let bundle = bundle_of(&database.path);
    assert_eq!(bundle.last_assistant, "Output was long.");
    assert!(serde_json::to_string(&bundle).unwrap().len() < 1_000);
}

#[test]
fn synthetic_text_is_read_from_the_text_field_only() {
    let mut database = Database::new();
    database.add(
        SESSION,
        "synthetic",
        json!({"payload": {"text": "from payload"}}),
    );
    database.add(SESSION, "synthetic", json!({"text": "from text"}));
    let texts: Vec<_> = bundle_of(&database.path)
        .recent
        .into_iter()
        .map(|message| message.text)
        .collect();
    assert_eq!(texts, ["from text"]);
}

#[test]
fn a_session_without_conversation_rows_yields_an_error_not_an_empty_bundle() {
    let mut database = Database::new();
    database.add("ses_other", "user", json!({"text": "Someone else's work"}));
    assert!(
        stop_bundle(
            &database.path,
            SESSION,
            Some(&todo_item("Fix login", "")),
            Vec::new()
        )
        .is_err()
    );
}

#[test]
fn a_finished_question_form_is_not_open() {
    let mut database = Database::new();
    database.add(SESSION, "assistant", json!({"content": [
        {"type": "tool", "name": "question", "state": {"status": "completed", "input": {"questions": [
            {"header": "Approach", "question": "Which approach?", "options": [{"label": "Small"}]}]}}}]}));
    assert!(bundle_of(&database.path).question.is_none());
}

#[test]
fn a_bundle_respects_its_size_caps() {
    let mut database = Database::new();
    for index in 0..30 {
        let marker = format!("{index:02}");
        database.user(&format!("{marker}{}", "é".repeat(5_000)));
    }
    database.assistant(&"a".repeat(5_000));
    let bundle = bundle_of(&database.path);
    let lengths: Vec<usize> = bundle
        .recent
        .iter()
        .map(|message| message.text.chars().count())
        .collect();
    assert!(lengths.len() <= 20, "{lengths:?}");
    assert!(lengths.iter().all(|length| *length <= 2_000), "{lengths:?}");
    assert!(lengths.iter().sum::<usize>() <= 16_000, "{lengths:?}");
    assert_eq!(bundle.last_assistant.chars().count(), 2_000);
    assert!(bundle.last_assistant.ends_with('…'));
    let newest = bundle.recent.last().unwrap();
    assert_eq!(
        (newest.role.as_str(), newest.text.chars().count()),
        ("assistant", 2_000)
    );
    assert!(
        bundle
            .recent
            .iter()
            .all(|message| message.text.ends_with('…')),
        "truncation must be visible"
    );
}

#[test]
fn only_the_last_twenty_messages_are_kept() {
    let mut database = Database::new();
    for index in 0..25 {
        database.user(&format!("message {index}"));
    }
    let bundle = bundle_of(&database.path);
    assert_eq!(bundle.recent.len(), 20);
    assert_eq!(bundle.recent[0].text, "message 5");
    assert_eq!(bundle.recent[19].text, "message 24");
}

#[test]
fn a_missing_database_yields_an_error_not_an_empty_bundle() {
    let sandbox = common::sandbox::Sandbox::new();
    let error = stop_bundle(
        &sandbox.home.join("absent.db"),
        SESSION,
        Some(&todo_item("Fix login", "")),
        Vec::new(),
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::Unavailable);
}

#[test]
fn an_unreadable_database_yields_an_error_not_an_empty_bundle() {
    let sandbox = common::sandbox::Sandbox::new();
    let path = sandbox.home.join("garbage.db");
    std::fs::write(&path, "not a database at all, just text").unwrap();
    assert!(
        stop_bundle(
            &path,
            SESSION,
            Some(&todo_item("Fix login", "")),
            Vec::new()
        )
        .is_err()
    );
}

fn project_with(files: &[(&str, &str)]) -> (common::sandbox::Sandbox, PathBuf) {
    let sandbox = common::sandbox::Sandbox::new();
    let root = sandbox.home.join("project");
    std::fs::create_dir_all(&root).unwrap();
    for (name, content) in files {
        std::fs::write(root.join(name), content).unwrap();
    }
    (sandbox, root)
}

#[test]
fn a_bundle_carries_the_projects_instruction_files_masked_and_capped() {
    let long = format!(
        "Use GITHUB_TOKEN={TOKEN} for nothing.\n{}",
        "line of rules\n".repeat(4_000)
    );
    assert!(long.len() > 50_000);
    let (_sandbox, root) =
        project_with(&[("AGENTS.md", &long), ("CLAUDE.md", "Be brief\u{202e}.")]);
    let (instructions, warnings) = instruction_files(&root);
    assert!(warnings.is_empty(), "{warnings:?}");
    let paths: Vec<&str> = instructions.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(paths, ["AGENTS.md", "CLAUDE.md"]);
    let agents = &instructions[0].text;
    assert!(
        !agents.contains("ghp_0123") && agents.contains("[redacted]"),
        "{agents}"
    );
    assert_eq!(agents.chars().count(), 4_000);
    assert!(agents.ends_with('…'));
    assert_eq!(instructions[1].text, "Be brief\\u202e.");
    let mut database = Database::new();
    database.assistant("Which option?");
    let bundle = stop_bundle(
        &database.path,
        SESSION,
        Some(&todo_item("Fix login", "")),
        instructions.clone(),
    )
    .unwrap();
    assert_eq!(bundle.instructions, instructions);
    let instruction_bytes = serde_json::to_string(&bundle.instructions).unwrap().len();
    assert!(instruction_bytes <= 2 * 6_100, "{instruction_bytes}");
}

#[test]
fn instruction_files_have_their_own_budget_beside_the_conversation() {
    let mut database = Database::new();
    for _ in 0..25 {
        database.user(&"u".repeat(5_000));
        database.assistant(&"a".repeat(5_000));
    }
    let hidden = "\u{202e}\"\\".repeat(5_000);
    let (_sandbox, root) = project_with(&[("AGENTS.md", &hidden), ("CLAUDE.md", &hidden)]);
    let (instructions, _) = instruction_files(&root);
    let mut bundle = stop_bundle(
        &database.path,
        SESSION,
        Some(&todo_item(&"t".repeat(50_000), &"n".repeat(50_000))),
        instructions,
    )
    .unwrap();
    assert_eq!(bundle.instructions.len(), 2);
    assert!(
        bundle
            .instructions
            .iter()
            .all(|file| serde_json::to_string(&file.text).unwrap().len() <= 6_002)
    );
    assert!(serde_json::to_string(&bundle).unwrap().len() <= 16_000 + 12_100);
    bundle.instructions.clear();
    assert!(
        serde_json::to_string(&bundle).unwrap().len() <= 16_000,
        "the conversation keeps its own cap"
    );
}

#[test]
fn missing_and_symlinked_instruction_files_are_skipped() {
    let (sandbox, root) = project_with(&[]);
    let outside = sandbox.home.join("secret.md");
    std::fs::write(&outside, "outside the project").unwrap();
    std::os::unix::fs::symlink(&outside, root.join("CLAUDE.md")).unwrap();
    let (instructions, warnings) = instruction_files(&root);
    assert_eq!(
        (instructions, warnings),
        (Vec::<InstructionFile>::new(), Vec::<String>::new())
    );
    assert_eq!(
        instruction_files(&sandbox.home.join("absent")),
        (Vec::new(), Vec::new())
    );
}

#[test]
fn an_unreadable_instruction_file_is_a_warning_not_a_failed_bundle() {
    let (_sandbox, root) = project_with(&[("CLAUDE.md", "Be brief.")]);
    std::fs::create_dir(root.join("AGENTS.md")).unwrap();
    let (instructions, warnings) = instruction_files(&root);
    assert_eq!(
        instructions,
        [InstructionFile {
            path: "CLAUDE.md".into(),
            text: "Be brief.".into()
        }]
    );
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("AGENTS.md"), "{warnings:?}");
    let (_, warnings) = instruction_files(Path::new("relative/project"));
    assert_eq!(
        warnings.len(),
        1,
        "a project root must be absolute: {warnings:?}"
    );
}

#[test]
fn only_messages_theo_typed_are_marked_as_his() {
    let mut database = Database::new();
    let item = |id: &str, text: &str| {
        Item(json!({"id": id, "text": text, "project": "agent-cockpit", "status": "done", "result": "Fixed."}).as_object().unwrap().clone())
    };
    database.user(&cockpit::todo::with_todo_note("Fix login", "1d3fd318"));
    database.assistant("Which option?");
    database.user("Keep commits small.");
    database.user(&cockpit::todo::with_todo_note(
        &format!("Always {}", "b".repeat(3_000)),
        "2e4fa419",
    ));
    database.user(&cockpit::errors::prompt("boom", &[]));
    database.user(&cockpit::todo::done_prompt(&item("3a5fb52a", "Filed item")));
    database.user(&cockpit::todo::retry_prompt(&item(
        "4b6fc63b",
        "Blocked item",
    )));
    database.user(&cockpit::teams::recruit_brief_text(
        cockpit::teams::Role::Designer,
        "developer",
        "Draw it.",
    ));
    database.add(
        SESSION,
        "synthetic",
        json!({"text": "Team message: use B."}),
    );
    let flags: Vec<(String, bool)> = bundle_of(&database.path)
        .recent
        .into_iter()
        .map(|message| (message.role, message.from_theo))
        .collect();
    let expected = [
        ("user", false),
        ("assistant", false),
        ("user", true),
        ("user", false),
        ("user", false),
        ("user", false),
        ("user", false),
        ("user", false),
        ("synthetic", false),
    ];
    assert_eq!(
        flags,
        expected.map(|(role, is_theo)| (role.to_string(), is_theo))
    );
}

#[test]
fn the_first_message_of_a_session_is_never_taken_for_theos() {
    let mut database = Database::new();
    database.user("Review pull request #12 of this repository.");
    database.assistant("Done?");
    assert!(
        bundle_of(&database.path)
            .recent
            .iter()
            .all(|message| !message.from_theo)
    );
}
