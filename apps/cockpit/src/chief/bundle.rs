//! The masked account of a stopped agent that the Chief reads: conversation text only.
//!
//! Tool outputs, reasoning and attachments never enter a bundle. The one exception is the input of
//! the running `question` tool, which is the form the agent is waiting on.
//!
//! Every limit below is a limit on the serialised JSON the Chief receives, not only on characters:
//! escaped text costs more bytes than it has characters, and the conversation shares one budget.
//! The project's instruction files have their own, beside it.

use crate::approval::{escape_hiding, redact_secrets};
use crate::teams::{ErrorCode, TeamError};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

const MESSAGES_MAX: usize = 20;
const MESSAGE_CHARS_MAX: usize = 2_000;
/// The serialised size of the whole bundle.
const BUNDLE_BYTES_MAX: usize = 16_000;
const ROWS_SCANNED_MAX: i64 = 200;
/// Stored bytes read before older rows are left unread; the newest row is always read.
const SCANNED_BYTES_MAX: i64 = 16 * 1024 * 1024;
/// What SQLite hands over per extracted conversation text.
const EXTRACTED_CHARS_MAX: i64 = 65_536;
const FORM_OPTIONS_MAX: usize = 10;
const HEADER_LIMIT: Limit = Limit { chars: 200, bytes: 400 };
const QUESTION_LIMIT: Limit = Limit { chars: 1_000, bytes: 2_000 };
const OPTION_LIMIT: Limit = Limit { chars: 200, bytes: 300 };
const LAST_ASSISTANT_LIMIT: Limit = Limit { chars: MESSAGE_CHARS_MAX, bytes: 3_000 };
const TASK_TEXT_LIMIT: Limit = Limit { chars: 1_000, bytes: 1_500 };
const TASK_NOTES_LIMIT: Limit = Limit { chars: 1_500, bytes: 2_000 };
/// Each instruction file; two files add at most about 12 KB beside the conversation's 16,000 bytes.
const INSTRUCTION_LIMIT: Limit = Limit { chars: 4_000, bytes: 6_000 };
/// Read of an instruction file before it is masked and cut: far more than its limit keeps.
const INSTRUCTION_READ_BYTES: u64 = 64 * 1024;
const INSTRUCTION_FILES: [&str; 2] = ["AGENTS.md", "CLAUDE.md"];
const ELLIPSIS: char = '…';
const ELLIPSIS_BYTES: usize = 3;

#[derive(Clone, Copy)]
struct Limit {
    chars: usize,
    bytes: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StopBundle {
    pub question: Option<FormQuestion>,
    pub last_assistant: String,
    pub recent: Vec<BundleMessage>,
    pub task_text: String,
    pub task_notes: String,
    /// Stops recorded before instruction files were read carry none.
    #[serde(default)]
    pub instructions: Vec<InstructionFile>,
}

/// One of the project's agent instruction files, masked and cut; `path` is relative to the project root.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstructionFile {
    pub path: String,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormQuestion {
    pub header: String,
    pub question: String,
    pub options: Vec<String>,
    /// How many questions the form asks. Only the first is shown, so callers must refuse to answer
    /// a form where this is above 1.
    pub question_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleMessage {
    pub role: String,
    pub text: String,
    /// A user message Theo typed himself, as far as the host can tell (`is_theos_message`). Only
    /// these make a stop a source of Theo's habits.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub from_theo: bool,
}

/// Whether a user message, as extracted whole from the conversation, is Theo's own words: never the
/// session's first message (a launch prompt or brief, whoever started it), never one cut at extraction
/// (its ending cannot be checked), and never a prompt the cockpit types itself.
fn is_theos_message(text: &str, is_first: bool) -> bool {
    let is_host_prompt = crate::todo::is_todo_handoff(text) || crate::todo::is_done_or_retry_prompt(text)
        || crate::teams::is_team_brief(text) || crate::errors::is_handoff(text);
    !is_first && text.chars().count() < EXTRACTED_CHARS_MAX as usize && !is_host_prompt
}

fn unavailable(message: &str) -> TeamError { TeamError::new(ErrorCode::Unavailable, message) }

fn masked(text: &str) -> String {
    let (redacted, _) = redact_secrets(text);
    escape_hiding(&redacted, "", "")
}

/// Bytes `character` takes inside a serialised JSON string.
fn json_cost(character: char) -> usize {
    match character {
        '"' | '\\' | '\n' | '\r' | '\t' | '\u{8}' | '\u{c}' => 2,
        control if (control as u32) < 0x20 => 6,
        other => other.len_utf8(),
    }
}

fn json_cost_of(text: &str) -> usize { text.chars().map(json_cost).sum() }

/// `text` cut to the limits, with a trailing ellipsis when anything was cut.
fn bounded(text: String, chars: usize, bytes: usize) -> String {
    if text.chars().count() <= chars && json_cost_of(&text) <= bytes { return text; }
    let char_budget = chars.saturating_sub(1);
    let byte_budget = bytes.saturating_sub(ELLIPSIS_BYTES);
    let mut kept = String::new();
    let mut cost = 0;
    for character in text.chars().take(char_budget) {
        cost += json_cost(character);
        if cost > byte_budget { break; }
        kept.push(character);
    }
    kept.push(ELLIPSIS);
    kept
}

fn limited(text: &str, limit: Limit) -> String { bounded(masked(text), limit.chars, limit.bytes) }

fn open_question(form: &str) -> Result<Option<FormQuestion>, TeamError> {
    let form: Value = serde_json::from_str(form).map_err(|_| unavailable("conversation history contains an invalid form"))?;
    let Some(question_count) = form["count"].as_u64().filter(|count| *count > 0) else { return Ok(None) };
    let options = form["options"].as_array().map(|options| options.iter()
        .filter_map(Value::as_str).take(FORM_OPTIONS_MAX)
        .map(|label| limited(label, OPTION_LIMIT)).collect()).unwrap_or_default();
    Ok(Some(FormQuestion {
        header: limited(form["header"].as_str().unwrap_or_default(), HEADER_LIMIT),
        question: limited(form["question"].as_str().unwrap_or_default(), QUESTION_LIMIT),
        options,
        question_count: question_count as usize,
    }))
}

/// Shortens the oldest messages first until the serialised bundle fits; a message that would
/// shrink to nothing goes. The fixed parts are budgeted so that this always succeeds.
fn fit_to_budget(bundle: &mut StopBundle) -> Result<(), TeamError> {
    loop {
        let size = serde_json::to_string(&*bundle)?.len();
        if size <= BUNDLE_BYTES_MAX { return Ok(()); }
        if bundle.recent.is_empty() {
            return Err(TeamError::new(ErrorCode::Limit, "stop bundle exceeds its size budget"));
        }
        let excess = size - BUNDLE_BYTES_MAX;
        let oldest = &mut bundle.recent[0];
        let cost = json_cost_of(&oldest.text);
        if cost <= excess + ELLIPSIS_BYTES {
            bundle.recent.remove(0);
        } else {
            oldest.text = bounded(std::mem::take(&mut oldest.text), usize::MAX, cost - excess);
        }
    }
}

/// Newest first. SQLite extracts the conversation text and the running form input itself, so tool
/// outputs of any size never reach this process; the form is read only from the newest row.
const ROWS_SQL: &str = "
    select type, length(data),
        substr(case when type = 'assistant' then
            case when json_type(data, '$.content') = 'array' then
                (select group_concat(text, char(10) || char(10)) from
                    (select json_extract(part.value, '$.text') as text from json_each(data, '$.content') as part
                     where json_extract(part.value, '$.type') = 'text' order by part.key))
            end
        else json_extract(data, '$.text') end, 1, ?3),
        case when type = 'assistant' and json_type(data, '$.content') = 'array'
            and seq = (select max(seq) from session_message where session_id = ?1 and type in ('user', 'assistant', 'synthetic')) then
            (select json_object('count', json_array_length(input, '$.questions'),
                    'header', substr(json_extract(input, '$.questions[0].header'), 1, 1000),
                    'question', substr(json_extract(input, '$.questions[0].question'), 1, 5000),
                    'options', json((select json_group_array(label) from
                        (select substr(json_extract(choice.value, '$.label'), 1, 1000) as label
                         from json_each(input, '$.questions[0].options') as choice
                         where json_extract(choice.value, '$.label') is not null order by choice.key limit 10))))
             from (select json_extract(part.value, '$.state.input') as input from json_each(data, '$.content') as part
                   where json_extract(part.value, '$.type') = 'tool' and json_extract(part.value, '$.name') = 'question'
                     and json_extract(part.value, '$.state.status') = 'running' order by part.key desc limit 1))
        end,
        coalesce(seq = (select min(seq) from session_message where session_id = ?1 and type = 'user'), 0)
    from session_message
    where session_id = ?1 and type in ('user', 'assistant', 'synthetic') order by seq desc limit ?2";

/// The project's `AGENTS.md` and `CLAUDE.md`, read from `project_root` only: a symlink is skipped
/// rather than followed out of the project, and a missing file is skipped. Anything else that keeps a
/// file from being read is returned as a warning, so the stop is still recorded without it.
pub fn instruction_files(project_root: &Path) -> (Vec<InstructionFile>, Vec<String>) {
    if !project_root.is_absolute() {
        return (Vec::new(), vec![format!("project root {} is not absolute; instruction files not read", project_root.display())]);
    }
    let mut files = Vec::new();
    let mut warnings = Vec::new();
    for name in INSTRUCTION_FILES {
        let path = project_root.join(name);
        match read_instruction_file(&path) {
            Ok(Some(text)) => files.push(InstructionFile { path: name.into(), text }),
            Ok(None) => {},
            Err(error) => warnings.push(format!("instruction file {} not read: {error}", path.display())),
        }
    }
    (files, warnings)
}

fn read_instruction_file(path: &Path) -> std::io::Result<Option<String>> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
        Ok(metadata) if metadata.file_type().is_symlink() => return Ok(None),
        Ok(_) => {},
    }
    // O_NOFOLLOW: the file cannot turn into a symlink after the check; O_NONBLOCK: a FIFO cannot hang the daemon.
    let file = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(path)?;
    if !file.metadata()?.is_file() { return Err(std::io::Error::other("not a regular file")); }
    let mut content = Vec::new();
    file.take(INSTRUCTION_READ_BYTES + 1).read_to_end(&mut content)?;
    let is_cut = content.len() as u64 > INSTRUCTION_READ_BYTES;
    if is_cut {
        // Keep whole lines only: a token cut at the read limit would no longer look like one to the masking.
        content.truncate(content.iter().rposition(|byte| *byte == b'\n').unwrap_or(0));
    }
    let mut text = String::from_utf8_lossy(&content).into_owned();
    if is_cut { text.push(ELLIPSIS); }
    Ok(Some(limited(&text, INSTRUCTION_LIMIT)))
}

/// `instructions` come from `instruction_files`; they sit beside the conversation's budget, not inside it.
pub fn stop_bundle(database: &Path, session_id: &str, todo: &crate::todo::Item, instructions: Vec<InstructionFile>) -> Result<StopBundle, TeamError> {
    let connection = Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| unavailable("conversation history is unavailable"))?;
    connection.busy_timeout(Duration::from_millis(200)).map_err(|_| unavailable("conversation history is unavailable"))?;
    let mut statement = connection.prepare(ROWS_SQL).map_err(|_| unavailable("conversation history format is unavailable"))?;
    let rows = statement.query_map(rusqlite::params![session_id, ROWS_SCANNED_MAX, EXTRACTED_CHARS_MAX], |row|
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, Option<String>>(2)?, row.get::<_, Option<String>>(3)?, row.get::<_, bool>(4)?)))
        .map_err(|_| unavailable("conversation history could not be read"))?;
    let mut newest_first = Vec::new();
    let mut question = None;
    let mut rows_seen = 0;
    let mut scanned: i64 = 0;
    for row in rows {
        let (role, stored_bytes, text, form_input, is_first_user) = row.map_err(|_| unavailable("conversation history is invalid or could not be read"))?;
        scanned += stored_bytes;
        if rows_seen > 0 && scanned > SCANNED_BYTES_MAX { break; }
        rows_seen += 1;
        if let Some(input) = form_input { question = open_question(&input)?; }
        if newest_first.len() == MESSAGES_MAX { continue; }
        let text = text.unwrap_or_default();
        if text.trim().is_empty() { continue; }
        let from_theo = role == "user" && is_theos_message(&text, is_first_user);
        newest_first.push(BundleMessage { role, text: bounded(masked(&text), MESSAGE_CHARS_MAX, usize::MAX), from_theo });
    }
    if rows_seen == 0 {
        return Err(TeamError::new(ErrorCode::NotFound, "no conversation was recorded for this session"));
    }
    let last_assistant = newest_first.iter().find(|message| message.role == "assistant")
        .map(|message| bounded(message.text.clone(), LAST_ASSISTANT_LIMIT.chars, LAST_ASSISTANT_LIMIT.bytes)).unwrap_or_default();
    let mut bundle = StopBundle {
        question,
        last_assistant,
        recent: newest_first.into_iter().rev().collect(),
        task_text: limited(&todo.text(), TASK_TEXT_LIMIT),
        task_notes: limited(&todo.notes(), TASK_NOTES_LIMIT),
        instructions: Vec::new(),
    };
    fit_to_budget(&mut bundle)?;
    bundle.instructions = instructions;
    Ok(bundle)
}
