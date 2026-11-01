//! `cockpit todo`: the task list (#38). A launched session (one with `COCKPIT_LAUNCH_TOKEN`)
//! files, reads and works items on the agent socket, where the daemon identifies it; everyone
//! else is the owner, on the host-only socket. Proposals and blocker reports are a session's
//! only, so they always go to the agent socket. The daemon decides what each may do.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use cockpit_protocol::{
    Command, HexBytes, Reply, TaskDraftTargetView, TaskDraftView, TaskKindView, TaskNextView,
    TaskView,
};

use crate::agent::{agent_socket, exchange, failed, launch_token, socket_dir};
use crate::daemon::utf8_words;

/// What a verb sends, and whether it is a session's request by its nature.
pub(crate) struct Request {
    command: Command,
    is_session_only: bool,
    /// `image get` writes the bytes here instead of printing them.
    image_file: Option<PathBuf>,
}

/// The request for `cockpit todo <arguments>`, or the usage problem that stops it.
pub(crate) fn parse(arguments: &[OsString]) -> Result<Request, String> {
    let words = utf8_words(arguments)?;
    let token = launch_token();
    let (verb, rest) = words.split_first().ok_or("name a todo verb")?;
    let mut image_file = None;
    let mut is_session_only = false;
    let command = match (*verb, rest) {
        ("add", rest) => {
            let mut flags = Flags::parse(
                rest,
                &["--project", "--notes", "--kind", "--priority"],
                &["--after"],
                &[],
            )?;
            let after = flags.many("--after");
            Command::TaskAdd {
                launch_token: token,
                title: flags.one_positional("a title")?,
                project: flags.take("--project"),
                notes: flags.take("--notes").unwrap_or_default(),
                item_kind: flags.kind()?,
                priority: flags.number("--priority", "a priority")?,
                after: item_ids(&after)?,
            }
        }
        ("propose", rest) => {
            is_session_only = true;
            let mut flags =
                Flags::parse(rest, &["--why", "--project", "--notes", "--kind"], &[], &[])?;
            Command::TaskPropose {
                launch_token: token,
                title: flags.one_positional("a title")?,
                why: flags.required("--why", "why it matters")?,
                project: flags.take("--project"),
                notes: flags.take("--notes").unwrap_or_default(),
                item_kind: flags.kind()?,
            }
        }
        ("blocked", rest) => {
            is_session_only = true;
            let mut flags = Flags::parse(rest, &["--why", "--notes"], &[], &[])?;
            Command::TaskReportBlocker {
                launch_token: token,
                title: flags.one_positional("what you could not do")?,
                why: flags.required("--why", "what stopped you, with the exact error")?,
                notes: flags.take("--notes").unwrap_or_default(),
            }
        }
        ("list", rest) => {
            let mut flags = Flags::parse(rest, &["--project"], &[], &["--all"])?;
            let all = flags.switch("--all");
            flags.no_positional()?;
            Command::TaskList {
                launch_token: token,
                project: flags.take("--project"),
                all,
            }
        }
        ("show", [id]) => Command::TaskShow {
            launch_token: token,
            item: item_id(id)?,
        },
        ("edit", [id, rest @ ..]) => {
            let mut flags = Flags::parse(
                rest,
                &["--revision", "--title", "--notes", "--kind"],
                &[],
                &[],
            )?;
            flags.no_positional()?;
            let edit = Command::TaskEdit {
                item: item_id(id)?,
                intent_revision: flags.number("--revision", "a revision")?,
                title: flags.take("--title"),
                notes: flags.take("--notes"),
                item_kind: flags.kind()?,
            };
            if let Command::TaskEdit {
                title: None,
                notes: None,
                item_kind: None,
                ..
            } = edit
            {
                return Err("name at least one of --title, --notes, --kind".to_owned());
            }
            edit
        }
        ("priority", [id, level]) => Command::TaskPriority {
            item: item_id(id)?,
            priority: number(level, "a priority")?,
        },
        ("queue", [id]) => Command::TaskQueue {
            item: item_id(id)?,
            queued: true,
        },
        ("unqueue", [id]) => Command::TaskQueue {
            item: item_id(id)?,
            queued: false,
        },
        ("approve", [id]) => Command::TaskApprove { item: item_id(id)? },
        ("decline", [id]) => Command::TaskDecline { item: item_id(id)? },
        ("start", [id]) => Command::TaskStart {
            launch_token: token,
            item: item_id(id)?,
        },
        ("done", [id, rest @ ..]) => {
            let mut flags = Flags::parse(rest, &["--summary"], &[], &[])?;
            flags.no_positional()?;
            Command::TaskDone {
                launch_token: token,
                item: item_id(id)?,
                summary: flags.take("--summary"),
            }
        }
        ("reopen", [id]) => Command::TaskReopen { item: item_id(id)? },
        ("after", [id, on @ ..]) if !on.is_empty() => Command::TaskWait {
            item: item_id(id)?,
            on: item_ids(on)?,
        },
        ("unwait", [id, on @ ..]) if !on.is_empty() => Command::TaskUnwait {
            item: item_id(id)?,
            on: item_ids(on)?,
        },
        ("delete", [id]) => Command::TaskDelete {
            launch_token: token,
            item: item_id(id)?,
        },
        ("image", ["add", id, file]) => Command::TaskImageAdd {
            item: item_id(id)?,
            data: HexBytes(
                std::fs::read(file).map_err(|error| format!("cannot read {file}: {error}"))?,
            ),
        },
        ("image", ["get", id, image_number, file]) => {
            image_file = Some(PathBuf::from(file));
            Command::TaskImage {
                item: item_id(id)?,
                number: number(image_number, "an image number")?,
            }
        }
        ("draft", ["save", rest @ ..]) => {
            let mut flags = Flags::parse(
                rest,
                &["--new", "--title", "--notes", "--revision"],
                &[],
                &[],
            )?;
            Command::TaskDraftSave {
                target: draft_target(&mut flags)?,
                title: flags.take("--title").unwrap_or_default(),
                notes: flags.take("--notes").unwrap_or_default(),
                base_intent_revision: flags.number("--revision", "a revision")?,
            }
        }
        ("draft", ["discard", rest @ ..]) => {
            let mut flags = Flags::parse(rest, &["--new"], &[], &[])?;
            Command::TaskDraftDiscard {
                target: draft_target(&mut flags)?,
            }
        }
        ("drafts", []) => Command::TaskDrafts,
        _ => return Err(format!("unknown todo arguments: {}", words.join(" "))),
    };
    Ok(Request {
        command,
        is_session_only,
        image_file,
    })
}

/// Sends the request on the socket of whoever runs it and prints the answer.
pub(crate) fn run(request: Request) -> ExitCode {
    let is_session = request.is_session_only || launch_token().is_some();
    let socket = match socket(is_session) {
        Ok(socket) => socket,
        Err(code) => return code,
    };
    match (exchange(&socket, request.command), request.image_file) {
        (Ok(Reply::TaskImage { data, .. }), Some(file)) => write_image(&file, &data.0),
        (Ok(reply), _) => match render(&reply) {
            Some(text) => {
                out!("{text}");
                ExitCode::SUCCESS
            }
            None => failed(&socket, Ok(reply)),
        },
        (outcome, _) => failed(&socket, outcome),
    }
}

pub(crate) fn socket(is_session: bool) -> Result<PathBuf, ExitCode> {
    if is_session {
        agent_socket()
    } else {
        Ok(socket_dir()?.join(cockpit_config::DAEMON_SOCKET))
    }
}

fn write_image(file: &Path, bytes: &[u8]) -> ExitCode {
    match std::fs::write(file, bytes) {
        Ok(()) => {
            outln!("wrote {} bytes to {}", bytes.len(), file.display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("cockpit: cannot write {}: {error}", file.display());
            ExitCode::FAILURE
        }
    }
}

/// The reply as printed, or `None` when it is not one a todo verb gets.
fn render(reply: &Reply) -> Option<String> {
    Some(match reply {
        Reply::Task { task } => details(task),
        Reply::Tasks { tasks } => tasks.iter().map(line).collect(),
        Reply::TaskDeleted { item } => format!("deleted\t{item}\n"),
        Reply::TaskImageAdded { item, image } => format!(
            "item {item}: [Image {}] {} {} bytes\n",
            image.number, image.media_type, image.bytes
        ),
        Reply::TaskDraft { draft } => draft_line(draft),
        Reply::TaskDrafts { drafts } => drafts.iter().map(draft_line).collect(),
        Reply::TaskDraftDiscarded => "discarded\n".to_owned(),
        _ => return None,
    })
}

/// `id  project  status  priority  kind  title`, tab-separated.
fn line(task: &TaskView) -> String {
    format!(
        "{}\t{}\t{}\t{}\t{}\t{}\n",
        task.id,
        task.project,
        status(task),
        task.priority,
        task.kind.as_str(),
        task.title
    )
}

fn status(task: &TaskView) -> String {
    let mut status = task.status.as_str().to_owned();
    if task.is_queued {
        status.push_str(",queued");
    }
    if task.is_blocker_report {
        status.push_str(",blocker");
    }
    status
}

/// Every field of one item, then what has to happen for it to move.
fn details(task: &TaskView) -> String {
    let mut text = format!(
        "item {} (intent revision {}, state revision {})\n\
         project: {}\ntitle: {}\nstatus: {}\nkind: {}\npriority: {}\n",
        task.id,
        task.intent_revision,
        task.state_revision,
        task.project,
        task.title,
        status(task),
        task.kind.as_str(),
        task.priority
    );
    if let Some(session) = &task.filed_by {
        let from = task.filed_from.as_deref().unwrap_or("no project");
        text.push_str(&format!("filed by: {session} from {from}\n"));
    }
    if let Some(why) = &task.why {
        text.push_str(&format!("why: {why}\n"));
    }
    for wait in &task.waits {
        text.push_str(&format!(
            "after: {} {} ({})",
            wait.item,
            wait.title,
            wait.status.as_str()
        ));
        if let Some(summary) = &wait.summary {
            text.push_str(&format!(": {summary}"));
        }
        text.push('\n');
    }
    for image in &task.images {
        text.push_str(&format!(
            "[Image {}] {} {} bytes\n",
            image.number, image.media_type, image.bytes
        ));
    }
    if let Some(summary) = &task.summary {
        text.push_str(&format!("summary: {summary}\n"));
    }
    text.push_str(&format!("next: {}\n", next(&task.next)));
    if !task.notes.is_empty() {
        text.push_str(&format!("notes:\n{}\n", task.notes));
    }
    text
}

fn next(next: &TaskNextView) -> String {
    match next {
        TaskNextView::DecideProposal => "the owner approves or declines the proposal".to_owned(),
        TaskNextView::FinishDependencies { items } => {
            let items: Vec<String> = items.iter().map(i64::to_string).collect();
            format!(
                "items {} finish first, or the owner removes the wait",
                items.join(", ")
            )
        }
        TaskNextView::Queue => "the owner queues it, or a session starts it".to_owned(),
        TaskNextView::AwaitPlacement => "queued: it waits for a session to take it".to_owned(),
        TaskNextView::InProgress {
            session: Some(session),
        } => format!("session {session} works on it"),
        TaskNextView::InProgress { session: None } => "the owner works on it".to_owned(),
        TaskNextView::Nothing => "nothing: it is closed".to_owned(),
        TaskNextView::Unknown => "unknown to this cockpit version".to_owned(),
    }
}

/// `target  saved_at  title`, with `stale` when the item changed since the form opened.
fn draft_line(draft: &TaskDraftView) -> String {
    let target = match &draft.target {
        TaskDraftTargetView::New { project } => format!("new:{project}"),
        TaskDraftTargetView::Item { item } => format!("item:{item}"),
    };
    let stale = if draft.is_stale { "\tstale" } else { "" };
    format!("{target}\t{}{stale}\n", draft.title)
}

fn draft_target(flags: &mut Flags) -> Result<TaskDraftTargetView, String> {
    match (flags.take("--new"), flags.positional.as_slice()) {
        (Some(project), []) => Ok(TaskDraftTargetView::New { project }),
        (None, [id]) => Ok(TaskDraftTargetView::Item { item: item_id(id)? }),
        _ => Err("name the draft's item, or --new PROJECT for a new item's form".to_owned()),
    }
}

fn item_ids(words: &[impl AsRef<str>]) -> Result<Vec<i64>, String> {
    words.iter().map(|word| item_id(word.as_ref())).collect()
}

fn item_id(text: &str) -> Result<i64, String> {
    number(text, "an item id")
}

pub(crate) fn number(text: &str, what: &str) -> Result<i64, String> {
    text.parse().map_err(|_| format!("{text} is not {what}"))
}

/// Words split into positional ones, `--flag value` pairs and switches. `single` flags take
/// one value at most once, `repeated` ones any number of times; `switches` take none.
pub(crate) struct Flags {
    positional: Vec<String>,
    values: Vec<(String, String)>,
    switches: Vec<String>,
}

impl Flags {
    pub(crate) fn parse(
        words: &[&str],
        single: &[&str],
        repeated: &[&str],
        switches: &[&str],
    ) -> Result<Flags, String> {
        let mut flags = Flags {
            positional: Vec::new(),
            values: Vec::new(),
            switches: Vec::new(),
        };
        let mut words = words.iter();
        while let Some(word) = words.next() {
            match *word {
                flag if single.contains(&flag) || repeated.contains(&flag) => {
                    let value = words
                        .next()
                        .ok_or_else(|| format!("{flag} needs a value"))?;
                    if single.contains(&flag) && flags.values.iter().any(|(name, _)| name == flag) {
                        return Err(format!("{flag} given twice"));
                    }
                    flags.values.push((flag.to_owned(), (*value).to_owned()));
                }
                switch if switches.contains(&switch) => flags.switches.push(switch.to_owned()),
                flag if flag.starts_with("--") => {
                    return Err(format!("unknown argument {flag}"));
                }
                positional => flags.positional.push(positional.to_owned()),
            }
        }
        Ok(flags)
    }

    pub(crate) fn take(&mut self, flag: &str) -> Option<String> {
        let index = self.values.iter().position(|(name, _)| name == flag)?;
        Some(self.values.remove(index).1)
    }

    pub(crate) fn required(&mut self, flag: &str, what: &str) -> Result<String, String> {
        self.take(flag)
            .ok_or_else(|| format!("{flag} is required: {what}"))
    }

    pub(crate) fn number(&mut self, flag: &str, what: &str) -> Result<Option<i64>, String> {
        self.take(flag).map(|text| number(&text, what)).transpose()
    }

    fn kind(&mut self) -> Result<Option<TaskKindView>, String> {
        self.take("--kind")
            .map(|text| {
                TaskKindView::parse(&text).ok_or_else(|| {
                    format!("--kind {text}: use one of {}", TaskKindView::known_names())
                })
            })
            .transpose()
    }

    pub(crate) fn many(&mut self, flag: &str) -> Vec<String> {
        std::iter::from_fn(|| self.take(flag)).collect()
    }

    pub(crate) fn switch(&mut self, flag: &str) -> bool {
        let index = self.switches.iter().position(|name| name == flag);
        index.map(|index| self.switches.remove(index)).is_some()
    }

    pub(crate) fn one_positional(&mut self, what: &str) -> Result<String, String> {
        match self.positional.as_slice() {
            [one] => Ok(one.clone()),
            [] => Err(format!("name {what}")),
            _ => Err(format!("give {what} as one quoted argument")),
        }
    }

    pub(crate) fn has_values(&self) -> bool {
        !self.values.is_empty()
    }

    pub(crate) fn no_positional(&self) -> Result<(), String> {
        match self.positional.first() {
            Some(word) => Err(format!("unexpected argument {word}")),
            None => Ok(()),
        }
    }
}
