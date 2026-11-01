//! `cockpit workflow`: an item's workflow, its steps and the task packet (#39). A launched
//! session (one with `COCKPIT_LAUNCH_TOKEN`) works its step on the agent socket, where the
//! daemon identifies it; everyone else is the owner, on the host-only socket. Beginning and
//! checkpointing a step are a session's only, so they always go to the agent socket. The
//! daemon decides what each may do.

use std::ffi::OsString;
use std::process::ExitCode;

use cockpit_protocol::{
    Command, Reply, WorkflowBlockedActionView, WorkflowBlockedView, WorkflowHandoffView,
    WorkflowNextView, WorkflowPacketView, WorkflowPresetView, WorkflowStepView, WorkflowView,
};

use crate::agent::{exchange, failed, launch_token};
use crate::daemon::utf8_words;
use crate::todo::{Flags, number, socket};

const HANDOFF_FLAGS: [&str; 4] = ["--changed", "--checked", "--outcome", "--needs-human"];

/// What a verb sends, and whether it is a session's request by its nature.
pub(crate) struct Request {
    command: Command,
    is_session_only: bool,
}

/// The request for `cockpit workflow <arguments>`, or the usage problem that stops it.
pub(crate) fn parse(arguments: &[OsString]) -> Result<Request, String> {
    let words = utf8_words(arguments)?;
    let token = launch_token();
    let (verb, rest) = words.split_first().ok_or("name a workflow verb")?;
    let mut is_session_only = false;
    let command = match (*verb, rest) {
        ("presets", []) => Command::WorkflowPresets,
        ("start", [item, rest @ ..]) => {
            let mut flags = Flags::parse(rest, &["--preset"], &[], &[])?;
            flags.no_positional()?;
            Command::WorkflowStart {
                item: number(item, "an item id")?,
                preset: flags.take("--preset"),
            }
        }
        ("list", rest) => {
            let mut flags = Flags::parse(rest, &["--project"], &[], &["--all"])?;
            let all = flags.switch("--all");
            flags.no_positional()?;
            Command::WorkflowList {
                launch_token: token,
                project: flags.take("--project"),
                all,
            }
        }
        ("show", [id]) => Command::WorkflowShow {
            launch_token: token,
            workflow: workflow_id(id)?,
        },
        ("packet", [id]) => Command::WorkflowPacket {
            launch_token: token,
            workflow: workflow_id(id)?,
        },
        ("begin", [id]) => {
            is_session_only = true;
            Command::WorkflowBegin {
                launch_token: token,
                workflow: workflow_id(id)?,
            }
        }
        ("checkpoint", [id, note]) => {
            is_session_only = true;
            Command::WorkflowCheckpoint {
                launch_token: token,
                workflow: workflow_id(id)?,
                note: (*note).to_owned(),
            }
        }
        ("complete", [id, rest @ ..]) => {
            let mut flags = Flags::parse(rest, &HANDOFF_FLAGS, &["--evidence", "--waive"], &[])?;
            flags.no_positional()?;
            let waive = flags.many("--waive");
            Command::WorkflowComplete {
                launch_token: token,
                workflow: workflow_id(id)?,
                handoff: handoff(&mut flags)?,
                waive,
            }
        }
        ("release", [id, rest @ ..]) => {
            let mut flags = Flags::parse(rest, &HANDOFF_FLAGS, &["--evidence"], &[])?;
            flags.no_positional()?;
            let has_handoff = flags.has_values();
            Command::WorkflowRelease {
                launch_token: token,
                workflow: workflow_id(id)?,
                handoff: has_handoff.then(|| handoff(&mut flags)).transpose()?,
            }
        }
        ("block", [id, action, rest @ ..]) => {
            let mut flags = Flags::parse(rest, &["--detail"], &[], &[])?;
            flags.no_positional()?;
            Command::WorkflowBlock {
                launch_token: token,
                workflow: workflow_id(id)?,
                action: WorkflowBlockedActionView::parse(action).ok_or_else(|| {
                    format!(
                        "{action}: use one of {}",
                        WorkflowBlockedActionView::known_names()
                    )
                })?,
                detail: flags.required("--detail", "what the blocker is")?,
            }
        }
        ("unblock", [id]) => Command::WorkflowUnblock {
            launch_token: token,
            workflow: workflow_id(id)?,
        },
        ("cancel", [id]) => Command::WorkflowCancel {
            workflow: workflow_id(id)?,
        },
        _ => return Err(format!("unknown workflow arguments: {}", words.join(" "))),
    };
    Ok(Request {
        command,
        is_session_only,
    })
}

/// Sends the request on the socket of whoever runs it and prints the answer.
pub(crate) fn run(request: Request) -> ExitCode {
    let is_session = request.is_session_only || launch_token().is_some();
    let socket = match socket(is_session) {
        Ok(socket) => socket,
        Err(code) => return code,
    };
    match exchange(&socket, request.command) {
        Ok(reply) => match render(&reply) {
            Some(text) => {
                out!("{text}");
                ExitCode::SUCCESS
            }
            None => failed(&socket, Ok(reply)),
        },
        outcome => failed(&socket, outcome),
    }
}

/// The handoff the flags give: what changed, what was checked and the outcome are required.
fn handoff(flags: &mut Flags) -> Result<WorkflowHandoffView, String> {
    Ok(WorkflowHandoffView {
        changed: flags.required("--changed", "what changed")?,
        checked: flags.required("--checked", "the exact result checked")?,
        outcome: flags.required("--outcome", "what passed or failed")?,
        needs_human: flags.take("--needs-human").unwrap_or_default(),
        evidence: flags.many("--evidence"),
    })
}

/// Free text a session or the owner wrote, its later lines indented so none reads as a
/// field line of its own (`next:` in a checkpoint note).
fn prose(text: &str) -> String {
    text.replace('\n', "\n  ")
}

fn workflow_id(text: &str) -> Result<i64, String> {
    number(text, "a workflow id")
}

/// The reply as printed, or `None` when it is not one a workflow verb gets.
fn render(reply: &Reply) -> Option<String> {
    Some(match reply {
        Reply::Workflow { workflow } => details(workflow),
        Reply::Workflows { workflows } => workflows.iter().map(line).collect(),
        Reply::WorkflowPacket { packet } => packet_text(packet),
        Reply::WorkflowPresets { presets } => presets.iter().map(preset_text).collect(),
        _ => return None,
    })
}

/// `id  project  item  preset  status  next`, tab-separated.
fn line(workflow: &WorkflowView) -> String {
    format!(
        "{}\t{}\t{}\t{}\t{}\t{}\n",
        workflow.id,
        workflow.project,
        workflow.item,
        workflow.preset,
        workflow.status.as_str(),
        next(&workflow.next)
    )
}

fn details(workflow: &WorkflowView) -> String {
    let mut text = format!(
        "workflow {} on item {} (intent revision {})\nproject: {}\npreset: {} v{}\nstatus: {}\n",
        workflow.id,
        workflow.item,
        workflow.intent_revision,
        workflow.project,
        workflow.preset,
        workflow.preset_version,
        workflow.status.as_str()
    );
    if let Some(blocked) = &workflow.blocked {
        text.push_str(&blocked_line(blocked));
    }
    for step in &workflow.steps {
        text.push_str(&step_line(step));
    }
    text.push_str(&format!("next: {}\n", next(&workflow.next)));
    text
}

fn blocked_line(blocked: &WorkflowBlockedView) -> String {
    format!(
        "blocked: {}: {}\n",
        blocked.action.as_str(),
        prose(&blocked.detail)
    )
}

/// `step N name (status, session) checkpoint` on one line.
fn step_line(step: &WorkflowStepView) -> String {
    let mut text = format!(
        "step {} {} ({}",
        step.position,
        step.name,
        step.status.as_str()
    );
    if let Some(session) = &step.session {
        text.push_str(&format!(", {session}"));
    }
    text.push(')');
    if let Some(checkpoint) = &step.checkpoint {
        text.push_str(&format!(": {}", prose(&checkpoint.note)));
    }
    text.push('\n');
    text
}

/// What a session needs to work its step: goal, scope, step, what came before, what is next.
fn packet_text(packet: &WorkflowPacketView) -> String {
    let goal = &packet.goal;
    let mut text = format!(
        "workflow {} ({} v{}, {})\nproject: {}\ngoal: item {} {} ({}, {})\n",
        packet.workflow,
        packet.preset,
        packet.preset_version,
        packet.status.as_str(),
        packet.project,
        goal.item,
        goal.title,
        goal.kind.as_str(),
        goal.item_status.as_str()
    );
    if goal.intent_changed {
        text.push_str(&format!(
            "intent changed: revision {} since the workflow started on {}\n",
            goal.intent_revision, goal.started_on_intent
        ));
    }
    if let Some(step) = &packet.step {
        text.push_str(&step_line(step));
        text.push_str(&format!("purpose: {}\n", step.purpose));
        if !step.requirements.is_empty() {
            text.push_str(&format!("requires: {}\n", step.requirements.join(", ")));
        }
    }
    if let Some(blocked) = &packet.blocked {
        text.push_str(&blocked_line(blocked));
    }
    for decision in &packet.decisions {
        text.push_str(&format!("decided: {} {}", decision.item, decision.title));
        if let Some(summary) = &decision.summary {
            text.push_str(&format!(": {}", prose(summary)));
        }
        text.push('\n');
    }
    for record in &packet.handoffs {
        text.push_str(&format!(
            "handoff from step {} {} ({}): changed: {}; checked: {}; outcome: {}\n",
            record.position,
            record.step,
            record.kind,
            prose(&record.handoff.changed),
            prose(&record.handoff.checked),
            prose(&record.handoff.outcome)
        ));
        if !record.handoff.needs_human.is_empty() {
            text.push_str(&format!(
                "needs the human: {}\n",
                prose(&record.handoff.needs_human)
            ));
        }
    }
    text.push_str(&format!("next: {}\n", next(&packet.next)));
    if !goal.notes.is_empty() {
        text.push_str(&format!("notes:\n{}\n", goal.notes));
    }
    text
}

/// `name vN: summary`, then one line per step with its doer and requirements.
fn preset_text(preset: &WorkflowPresetView) -> String {
    let mut text = format!("{} v{}: {}\n", preset.name, preset.version, preset.summary);
    for (index, step) in preset.steps.iter().enumerate() {
        text.push_str(&format!("  {} {} ({})", index + 1, step.name, step.doer));
        if !step.requirements.is_empty() {
            text.push_str(&format!(" requires {}", step.requirements.join(", ")));
        }
        text.push('\n');
    }
    text
}

fn next(next: &WorkflowNextView) -> String {
    match next {
        WorkflowNextView::Unblock { action } => format!("{} first", action.as_str()),
        WorkflowNextView::AwaitPlacement { position } => {
            format!("step {position} waits for a session to begin it")
        }
        WorkflowNextView::OwnerCompletes { position } => {
            format!("the owner completes step {position}")
        }
        WorkflowNextView::InProgress {
            position,
            session: Some(session),
        } => format!("session {session} works on step {position}"),
        WorkflowNextView::InProgress {
            position,
            session: None,
        } => format!("step {position} is in progress"),
        WorkflowNextView::Nothing => "nothing: it is closed".to_owned(),
        WorkflowNextView::Unknown => "unknown to this cockpit version".to_owned(),
    }
}
