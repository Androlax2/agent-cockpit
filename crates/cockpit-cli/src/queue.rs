//! `cockpit queue`: why each open item waits, and a scheduling pass on demand, served by the
//! daemon (#43).

use std::ffi::OsString;
use std::process::ExitCode;

use cockpit_protocol::{
    Command, DemandView, QueueExplanationView, QueuePlacementView, QueueStepView, Reply,
};

use crate::daemon::{CONTROL_DEADLINE, request, unexpected, utf8_words};

/// A `queue` verb's command, or the usage problem that stops it.
pub fn parse(arguments: &[OsString]) -> Result<Command, String> {
    let words = utf8_words(arguments)?;
    Ok(match words.as_slice() {
        ["explain"] => Command::QueueExplain { item: None },
        ["explain", item] => Command::QueueExplain {
            item: Some(
                item.parse()
                    .map_err(|_| format!("{item}: not an item number"))?,
            ),
        },
        ["dispatch"] => Command::QueueDispatch,
        _ => return Err("unknown queue arguments".to_owned()),
    })
}

/// Sends `command` and prints what it answers. Exit 1 when the daemon refuses.
pub fn run(command: Command) -> ExitCode {
    let reply = match request(command, CONTROL_DEADLINE) {
        Ok(reply) => reply,
        Err(code) => return code,
    };
    match &reply {
        Reply::QueueExplanations { explanations } if explanations.is_empty() => {
            outln!("nothing waits");
        }
        Reply::QueueExplanations { explanations } => {
            let blocks: Vec<String> = explanations.iter().map(explanation_text).collect();
            out!("{}", blocks.join("\n"));
        }
        Reply::QueueDispatched {
            placed,
            ended,
            waiting,
        } => out!("{}", dispatched_text(placed, ended, *waiting)),
        _ => return unexpected(&reply),
    }
    ExitCode::SUCCESS
}

/// One block per item: what waits, why, what limits it, each session's refusal, what it
/// would charge, how long it waited, and the action with who takes it.
fn explanation_text(explanation: &QueueExplanationView) -> String {
    let mut text = format!(
        "item {} {}: {}\n",
        explanation.item, explanation.project, explanation.title
    );
    if let Some(step) = &explanation.step {
        text.push_str(&format!("  waits as {}\n", step_text(step)));
    }
    text.push_str(&format!("  {}: {}\n", explanation.reason, explanation.why));
    if let Some(limit) = &explanation.limited_by {
        text.push_str(&format!("  limited by {limit}\n"));
    }
    if let Some(behind) = explanation.behind {
        text.push_str(&format!("  behind item {behind}\n"));
    }
    for note in &explanation.sessions {
        text.push_str(&format!("  session {}: {}\n", note.session, note.why));
    }
    text.push_str(&format!(
        "  priority {} + {} aging\tcharge {}",
        explanation.priority,
        explanation.aging_boost,
        demand_text(explanation.demand)
    ));
    if explanation.headroom_percent > 0 {
        text.push_str(&format!(
            " ({}% startup headroom)",
            explanation.headroom_percent
        ));
    }
    text.push('\n');
    if let Some(since) = explanation.waiting_since {
        text.push_str(&format!(
            "  waited {}\t",
            duration_text(explanation.evaluated_at - since)
        ));
    } else {
        text.push_str("  ");
    }
    text.push_str(&format!(
        "next pass in {}\n",
        duration_text(explanation.next_evaluation_at - explanation.evaluated_at)
    ));
    text.push_str(&format!(
        "  action ({}): {}\n",
        explanation.action_by, explanation.action
    ));
    text
}

fn dispatched_text(placed: &[QueuePlacementView], ended: &[i64], waiting: u64) -> String {
    let mut text = String::new();
    for placement in placed {
        let unit = match &placement.step {
            Some(step) => format!("{} of item {}", step_text(step), placement.item),
            None => format!("item {}", placement.item),
        };
        text.push_str(&format!(
            "placed {unit} ({}) on session {}\tcharge {}\n",
            placement.project,
            placement.session,
            demand_text(placement.demand)
        ));
    }
    for placement in ended {
        text.push_str(&format!("ended placement {placement}\n"));
    }
    match waiting {
        0 => text.push_str("nothing waits\n"),
        1 => text.push_str("1 item waits: `cockpit queue explain` says why\n"),
        count => text.push_str(&format!(
            "{count} items wait: `cockpit queue explain` says why\n"
        )),
    }
    text
}

fn step_text(step: &QueueStepView) -> String {
    format!(
        "step {} {} of workflow run {}",
        step.position, step.name, step.run
    )
}

fn demand_text(demand: DemandView) -> String {
    format!("{} millicores {} MiB", demand.cpu_millis, demand.memory_mib)
}

/// Whole minutes from a minute up, seconds below.
fn duration_text(seconds: i64) -> String {
    if seconds >= 60 {
        format!("{} min", seconds / 60)
    } else {
        format!("{seconds} s")
    }
}
