//! `ask guide`: steps an agent needs the owner to do by hand, shown one at a time. The owner
//! answers each step (with text when it asks a question), or says where they are stuck; the
//! agent gets the answers, or the stuck step and its note, back.
//!
//! The format is the current `ask-to-guide`'s: `# Title`, `## Section`, one step per line,
//! and under a step indented `open: <url>`, `copy: <value>`, `ask: <question>` or
//! `run: <command>`. A `run:` step is not run by answering it: answering files the command
//! as an `ask run` request of the guide's session, which the owner approves like any other.

use std::path::PathBuf;

use cockpit_authorization::SessionId;
use cockpit_turso::WriteTransaction;
use cockpit_turso::sql::{Connection, Row};

use crate::display::is_outside_project;
use crate::error::invalid;
use crate::runs::{
    EXPIRES_AFTER_SECONDS, Project, REASON_MAX_CHARS, RunAsk, RunId, insert_run, may_ask,
    validate_command,
};
use crate::{Error, Refusal};

const GUIDE_MAX_BYTES: usize = 64 * 1024;
const GUIDE_MAX_STEPS: usize = 100;
const LINE_MAX_CHARS: usize = 1000;
const ANSWER_MAX_CHARS: usize = 1000;

const SELECT_GUIDES: &str = "SELECT id, session, title, cwd, project, project_root, state,
        current_step, stuck_note, asked_at, expires_at
     FROM approvals_guides";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GuideId(pub i64);

/// One step as the agent wrote it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Step {
    /// The `## Section` the step is under, if any.
    pub section: Option<String>,
    pub text: String,
    pub open: Vec<String>,
    pub copy: Vec<String>,
    pub ask: Option<String>,
    pub run: Option<String>,
}

/// A parsed guide: its title and steps, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Guide {
    pub title: String,
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuideState {
    Asking,
    /// Asking, but nobody polled for it within [`EXPIRES_AFTER_SECONDS`]. Never stored.
    Expired,
    /// Every step answered.
    Done,
    /// The owner is stuck at the current step; the note says why.
    Stuck,
    /// The owner closed the guide without finishing it.
    Closed,
}

impl GuideState {
    pub fn as_str(self) -> &'static str {
        match self {
            GuideState::Asking => "asking",
            GuideState::Expired => "expired",
            GuideState::Done => "done",
            GuideState::Stuck => "stuck",
            GuideState::Closed => "closed",
        }
    }

    fn parse(text: &str) -> Option<GuideState> {
        [
            GuideState::Asking,
            GuideState::Done,
            GuideState::Stuck,
            GuideState::Closed,
        ]
        .into_iter()
        .find(|state| state.as_str() == text)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuideStep {
    pub step: Step,
    pub answer: Option<String>,
    /// The `ask run` request answering a `run:` step filed.
    pub run_request: Option<RunId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuideRequest {
    pub id: GuideId,
    pub session: SessionId,
    pub title: String,
    pub cwd: PathBuf,
    pub project: Option<Project>,
    pub state: GuideState,
    /// The step shown now, counted from 0; equal to the step count once done.
    pub current_step: usize,
    pub steps: Vec<GuideStep>,
    pub stuck_note: Option<String>,
    pub asked_at: i64,
    pub expires_at: i64,
}

impl GuideRequest {
    pub fn is_outside_project(&self) -> bool {
        is_outside_project(
            &self.cwd,
            self.project.as_ref().map(|project| project.root.as_path()),
        )
    }

    /// The step the owner is shown now, while the guide still asks.
    pub fn current(&self) -> Option<&GuideStep> {
        match self.state {
            GuideState::Asking => self.steps.get(self.current_step),
            _ => None,
        }
    }
}

/// Parses a guide in the `ask-to-guide` format, refusing what the owner could not follow.
pub fn parse_guide(text: &str) -> Result<Guide, Refusal> {
    if text.len() > GUIDE_MAX_BYTES {
        return Err(invalid(
            "guide",
            format!("is {} bytes; at most {GUIDE_MAX_BYTES}", text.len()),
        ));
    }
    let mut lines = text
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty());
    let title = match lines.next() {
        Some((_, line)) => match line.strip_prefix("# ") {
            Some(title) => checked_line(1, title)?,
            None => return Err(invalid("guide", "must start with a `# Title` line")),
        },
        None => return Err(invalid("guide", "is empty")),
    };
    let mut steps: Vec<Step> = Vec::new();
    let mut section: Option<String> = None;
    for (index, line) in lines {
        let number = index + 1;
        if let Some(heading) = line.strip_prefix("## ") {
            section = Some(checked_line(number, heading)?);
            continue;
        }
        if line.starts_with('#') {
            return Err(at_line(
                number,
                "only one `# Title` line, then `## Section` headings",
            ));
        }
        if !line.starts_with(char::is_whitespace) {
            steps.push(Step {
                section: section.clone(),
                text: checked_line(number, line)?,
                ..Step::default()
            });
            continue;
        }
        let Some(step) = steps.last_mut() else {
            return Err(at_line(number, "an indented line must follow a step"));
        };
        let (key, value) = line.trim().split_once(':').ok_or_else(|| {
            at_line(
                number,
                "an indented line is `open:`, `copy:`, `ask:` or `run:`",
            )
        })?;
        let value = checked_line(number, value)?;
        let single = |slot: &mut Option<String>, value: String| match slot.replace(value) {
            None => Ok(()),
            Some(_) => Err(at_line(number, format!("a step has one `{key}:` at most"))),
        };
        match key {
            "open" => step.open.push(value),
            "copy" => step.copy.push(value),
            "ask" => single(&mut step.ask, value)?,
            "run" => {
                validate_command("run", &value)?;
                single(&mut step.run, value)?;
            }
            _ => {
                return Err(at_line(
                    number,
                    format!("`{key}:` is not one of open, copy, ask, run"),
                ));
            }
        }
    }
    if steps.is_empty() {
        return Err(invalid("guide", "has no step"));
    }
    if steps.len() > GUIDE_MAX_STEPS {
        return Err(invalid(
            "guide",
            format!("has {} steps; at most {GUIDE_MAX_STEPS}", steps.len()),
        ));
    }
    Ok(Guide { title, steps })
}

fn checked_line(number: usize, text: &str) -> Result<String, Refusal> {
    let text = text.trim();
    if text.is_empty() {
        return Err(at_line(number, "is empty"));
    }
    if text.chars().count() > LINE_MAX_CHARS {
        return Err(at_line(
            number,
            format!("is longer than {LINE_MAX_CHARS} characters"),
        ));
    }
    if text.contains('\0') {
        return Err(at_line(number, "contains a NUL character"));
    }
    Ok(text.to_owned())
}

fn at_line(number: usize, reason: impl Into<String>) -> Refusal {
    invalid("guide", format!("line {number}: {}", reason.into()))
}

/// Files `guide` for the owner after `cockpit-authorization` allows `session`
/// `run-commands` over itself. `cwd` is canonical; `run:` steps run there.
pub async fn ask_guide(
    write: &WriteTransaction<'_>,
    session: &SessionId,
    guide: &Guide,
    cwd: PathBuf,
    project: Option<Project>,
    now: i64,
) -> Result<GuideRequest, Error> {
    may_ask(write, session).await?;
    let id = write
        .prepare("SELECT coalesce(max(id), 0) + 1 FROM approvals_guides")
        .await?
        .query_row(())
        .await?
        .get::<i64>(0)?;
    let cwd_text = cwd.to_str().ok_or_else(|| invalid("cwd", "is not UTF-8"))?;
    let project_root = project
        .as_ref()
        .map(|project| {
            project
                .root
                .to_str()
                .ok_or_else(|| invalid("cwd", "the project root is not UTF-8"))
        })
        .transpose()?;
    write
        .execute(
            "INSERT INTO approvals_guides (id, session, title, cwd, project, project_root,
                state, current_step, step_count, asked_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'asking', 0, ?7, ?8, ?9)",
            (
                id,
                session.as_str(),
                guide.title.as_str(),
                cwd_text,
                project.as_ref().map(|project| project.name.clone()),
                project_root,
                i64::try_from(guide.steps.len()).expect("at most GUIDE_MAX_STEPS"),
                now,
                now + EXPIRES_AFTER_SECONDS,
            ),
        )
        .await?;
    for (position, step) in guide.steps.iter().enumerate() {
        write
            .execute(
                "INSERT INTO approvals_guide_steps
                    (guide, position, section, text, open, copy, ask, run)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                (
                    id,
                    i64::try_from(position).expect("at most GUIDE_MAX_STEPS"),
                    step.section.clone(),
                    step.text.as_str(),
                    step.open.join("\n"),
                    step.copy.join("\n"),
                    step.ask.clone(),
                    step.run.clone(),
                ),
            )
            .await?;
    }
    required(write, GuideId(id), now).await
}

/// Guides still asking, oldest first.
pub async fn open_guides(connection: &Connection, now: i64) -> Result<Vec<GuideRequest>, Error> {
    let mut rows = connection
        .query(
            &format!(
                "{SELECT_GUIDES} WHERE state = 'asking' AND expires_at > ?1 ORDER BY asked_at, id"
            ),
            [now],
        )
        .await?;
    let mut headers = Vec::new();
    while let Some(row) = rows.next().await? {
        headers.push(guide_from(&row, now)?);
    }
    let mut guides = Vec::new();
    for guide in headers {
        guides.push(with_steps(connection, guide).await?);
    }
    Ok(guides)
}

pub async fn guide_request(
    connection: &Connection,
    id: GuideId,
    now: i64,
) -> Result<Option<GuideRequest>, Error> {
    let mut rows = connection
        .query(&format!("{SELECT_GUIDES} WHERE id = ?1"), [id.0])
        .await?;
    let Some(row) = rows.next().await? else {
        return Ok(None);
    };
    let guide = guide_from(&row, now)?;
    Ok(Some(with_steps(connection, guide).await?))
}

/// The guide as its asking session sees it; asking about it keeps it from expiring.
pub async fn guide_status(
    write: &WriteTransaction<'_>,
    session: &SessionId,
    id: GuideId,
    now: i64,
) -> Result<GuideRequest, Error> {
    let guide = match guide_request(write, id, now).await? {
        Some(guide) if guide.session == *session => guide,
        _ => return Err(not_found(id).into()),
    };
    if guide.state != GuideState::Asking {
        return Ok(guide);
    }
    write
        .execute(
            "UPDATE approvals_guides SET expires_at = ?2 WHERE id = ?1",
            (id.0, now + EXPIRES_AFTER_SECONDS),
        )
        .await?;
    // The agent waits on the guide, not on the requests its `run:` steps filed; they stay
    // before the owner as long as the guide does.
    write
        .execute(
            "UPDATE approvals_runs SET expires_at = ?2
             WHERE state = 'asking' AND expires_at > ?3
               AND id IN (SELECT run_request FROM approvals_guide_steps WHERE guide = ?1)",
            (id.0, now + EXPIRES_AFTER_SECONDS, now),
        )
        .await?;
    required(write, id, now).await
}

/// The owner's answer to step `step`, the one shown now. A step that asks a question needs
/// `answer`; a `run:` step files its command as an `ask run` request of the guide's session.
pub async fn answer_step(
    write: &WriteTransaction<'_>,
    id: GuideId,
    step: usize,
    answer: Option<String>,
    now: i64,
) -> Result<GuideRequest, Error> {
    let guide = asking_at(write, id, step, now).await?;
    let shown = &guide.steps[step].step;
    let answer = answer.map(|answer| checked_answer(&answer)).transpose()?;
    if shown.ask.is_some() && answer.is_none() {
        return Err(invalid("answer", "this step asks a question; answer it").into());
    }
    let run_request = match &shown.run {
        Some(command) => {
            may_ask(write, &guide.session).await?;
            let ask = RunAsk {
                command: command.clone(),
                cwd: guide.cwd.clone(),
                reason: run_reason(&guide, step),
                project: guide.project.clone(),
            };
            Some(insert_run(write, &guide.session, &ask, now).await?.id)
        }
        None => None,
    };
    let position = i64::try_from(step).expect("at most GUIDE_MAX_STEPS");
    write
        .execute(
            "UPDATE approvals_guide_steps SET answer = ?3, run_request = ?4, answered_at = ?5
             WHERE guide = ?1 AND position = ?2",
            (id.0, position, answer, run_request.map(|run| run.0), now),
        )
        .await?;
    let is_last = step + 1 == guide.steps.len();
    write
        .execute(
            "UPDATE approvals_guides SET current_step = ?2,
                state = CASE WHEN ?3 THEN 'done' ELSE state END,
                ended_at = CASE WHEN ?3 THEN ?4 ELSE ended_at END
             WHERE id = ?1",
            (id.0, position + 1, is_last, now),
        )
        .await?;
    required(write, id, now).await
}

/// The owner is stuck at step `step`; `note` says where and why.
pub async fn report_stuck(
    write: &WriteTransaction<'_>,
    id: GuideId,
    step: usize,
    note: &str,
    now: i64,
) -> Result<GuideRequest, Error> {
    asking_at(write, id, step, now).await?;
    let note = checked_answer(note).map_err(|_| {
        invalid(
            "note",
            format!("must be 1 to {ANSWER_MAX_CHARS} characters without NUL"),
        )
    })?;
    write
        .execute(
            "UPDATE approvals_guides SET state = 'stuck', stuck_note = ?2, ended_at = ?3
             WHERE id = ?1",
            (id.0, note, now),
        )
        .await?;
    required(write, id, now).await
}

pub async fn close_guide(
    write: &WriteTransaction<'_>,
    id: GuideId,
    now: i64,
) -> Result<GuideRequest, Error> {
    let guide = asking(write, id, now).await?;
    write
        .execute(
            "UPDATE approvals_guides SET state = 'closed', ended_at = ?2 WHERE id = ?1",
            (guide.id.0, now),
        )
        .await?;
    required(write, id, now).await
}

/// The reason an `ask run` request filed by a `run:` step shows the owner.
fn run_reason(guide: &GuideRequest, step: usize) -> String {
    let reason = format!(
        "Step {} of the guide \"{}\": {}",
        step + 1,
        guide.title,
        guide.steps[step].step.text
    );
    reason.chars().take(REASON_MAX_CHARS).collect()
}

fn checked_answer(answer: &str) -> Result<String, Refusal> {
    let answer = answer.trim();
    let count = answer.chars().count();
    if count == 0 || count > ANSWER_MAX_CHARS || answer.contains('\0') {
        return Err(invalid(
            "answer",
            format!("must be 1 to {ANSWER_MAX_CHARS} characters without NUL"),
        ));
    }
    Ok(answer.to_owned())
}

async fn asking(
    write: &WriteTransaction<'_>,
    id: GuideId,
    now: i64,
) -> Result<GuideRequest, Error> {
    let guide = guide_request(write, id, now)
        .await?
        .ok_or_else(|| not_found(id))?;
    match guide.state {
        GuideState::Asking => Ok(guide),
        GuideState::Expired => Err(Refusal::Expired.into()),
        state => Err(Refusal::AlreadyDecided {
            state: state.as_str(),
        }
        .into()),
    }
}

/// The guide if it still asks and `step` is the step it shows now.
async fn asking_at(
    write: &WriteTransaction<'_>,
    id: GuideId,
    step: usize,
    now: i64,
) -> Result<GuideRequest, Error> {
    let guide = asking(write, id, now).await?;
    if guide.current_step != step {
        return Err(Refusal::StaleStep {
            current: guide.current_step,
        }
        .into());
    }
    Ok(guide)
}

async fn required(connection: &Connection, id: GuideId, now: i64) -> Result<GuideRequest, Error> {
    guide_request(connection, id, now)
        .await?
        .ok_or_else(|| Error::InvalidStoredRow {
            table: "approvals_guides",
            reason: format!("guide {} vanished inside its transaction", id.0),
        })
}

fn not_found(id: GuideId) -> Refusal {
    Refusal::NotFound {
        kind: "guide",
        id: id.0,
    }
}

fn guide_from(row: &Row, now: i64) -> Result<GuideRequest, Error> {
    let invalid_row = |reason: String| Error::InvalidStoredRow {
        table: "approvals_guides",
        reason,
    };
    let session =
        SessionId::new(row.get::<String>(1)?).map_err(|error| invalid_row(error.to_string()))?;
    let stored_state = row.get::<String>(6)?;
    let mut state = GuideState::parse(&stored_state)
        .ok_or_else(|| invalid_row(format!("state {stored_state:?}")))?;
    let expires_at: i64 = row.get(10)?;
    if state == GuideState::Asking && now >= expires_at {
        state = GuideState::Expired;
    }
    let project = match (row.get::<Option<String>>(4)?, row.get::<Option<String>>(5)?) {
        (Some(name), Some(root)) => Some(Project {
            name,
            root: PathBuf::from(root),
        }),
        _ => None,
    };
    let current_step: i64 = row.get(7)?;
    Ok(GuideRequest {
        id: GuideId(row.get(0)?),
        session,
        title: row.get(2)?,
        cwd: PathBuf::from(row.get::<String>(3)?),
        project,
        state,
        current_step: usize::try_from(current_step)
            .map_err(|_| invalid_row(format!("current step {current_step}")))?,
        steps: Vec::new(),
        stuck_note: row.get(8)?,
        asked_at: row.get(9)?,
        expires_at,
    })
}

async fn with_steps(
    connection: &Connection,
    mut guide: GuideRequest,
) -> Result<GuideRequest, Error> {
    let mut rows = connection
        .query(
            "SELECT section, text, open, copy, ask, run, answer, run_request
             FROM approvals_guide_steps WHERE guide = ?1 ORDER BY position",
            [guide.id.0],
        )
        .await?;
    while let Some(row) = rows.next().await? {
        let values = |column: usize| -> Result<Vec<String>, Error> {
            let joined: String = row.get(column)?;
            Ok(joined
                .split('\n')
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .collect())
        };
        guide.steps.push(GuideStep {
            step: Step {
                section: row.get(0)?,
                text: row.get(1)?,
                open: values(2)?,
                copy: values(3)?,
                ask: row.get(4)?,
                run: row.get(5)?,
            },
            answer: row.get(6)?,
            run_request: row.get::<Option<i64>>(7)?.map(RunId),
        });
    }
    Ok(guide)
}
