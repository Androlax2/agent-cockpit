//! `cockpit approval` and `cockpit guide`: the owner reads what agents ask and decides, on
//! the host-only socket (#34). Commands, directories and reasons are printed under the
//! display rules the daemon applied, never as the agent wrote them; steps count from 1.

use std::ffi::OsString;
use std::fmt::Write as _;
use std::process::ExitCode;

use cockpit_protocol::{Command, GuideStateView, GuideView, Reply, RunRequestView, ShownText};

/// The command a verb sends, or the usage problem that stops it.
pub(crate) fn parse(verb: &str, arguments: &[OsString]) -> Result<Command, String> {
    let words: Vec<&str> = arguments
        .iter()
        .map(|word| {
            word.to_str()
                .ok_or_else(|| format!("{}: not UTF-8", word.to_string_lossy()))
        })
        .collect::<Result<_, _>>()?;
    let command = match (verb, words.as_slice()) {
        ("approval", ["list"]) => Command::ApprovalQueue,
        ("approval", ["show", id]) => Command::ApprovalShow {
            request: id_of(id)?,
        },
        ("approval", ["approve", id, "--revision", revision, "--digest", digest])
        | ("approval", ["approve", id, "--digest", digest, "--revision", revision]) => {
            Command::ApprovalApprove {
                request: id_of(id)?,
                revision: id_of(revision)?,
                digest: (*digest).to_owned(),
            }
        }
        ("approval", ["decline", id]) => Command::ApprovalDecline {
            request: id_of(id)?,
        },
        ("approval", ["edit", id, "--revision", revision, command]) => Command::ApprovalEdit {
            request: id_of(id)?,
            revision: id_of(revision)?,
            command: (*command).to_owned(),
        },
        ("approval", ["input", id, text]) => Command::ApprovalInput {
            request: id_of(id)?,
            text: (*text).to_owned(),
        },
        ("guide", ["show", id]) => Command::GuideShow { guide: id_of(id)? },
        ("guide", ["answer", id, step]) => Command::GuideAnswer {
            guide: id_of(id)?,
            step: step_of(step)?,
            answer: None,
        },
        ("guide", ["answer", id, step, answer]) => Command::GuideAnswer {
            guide: id_of(id)?,
            step: step_of(step)?,
            answer: Some((*answer).to_owned()),
        },
        ("guide", ["stuck", id, step, note]) => Command::GuideStuck {
            guide: id_of(id)?,
            step: step_of(step)?,
            note: (*note).to_owned(),
        },
        ("guide", ["close", id]) => Command::GuideClose { guide: id_of(id)? },
        _ => return Err(format!("unknown {verb} arguments")),
    };
    Ok(command)
}

fn id_of(text: &str) -> Result<i64, String> {
    text.parse().map_err(|_| format!("{text} is not a number"))
}

/// Steps count from 1 here, from 0 on the wire.
fn step_of(text: &str) -> Result<usize, String> {
    match text.parse::<usize>() {
        Ok(step) if step >= 1 => Ok(step - 1),
        _ => Err(format!("{text} is not a step number (from 1)")),
    }
}

/// Sends `command` on the host-only socket and prints the answer.
pub(crate) fn run(command: Command) -> ExitCode {
    let socket = match crate::agent::socket_dir() {
        Ok(dir) => dir.join(cockpit_config::DAEMON_SOCKET),
        Err(code) => return code,
    };
    match crate::agent::exchange(&socket, command) {
        Ok(Reply::ApprovalQueue { runs, guides }) => {
            for run in &runs {
                println!("{}", run_line(run));
            }
            for guide in &guides {
                println!("{}", guide_line(guide));
            }
            ExitCode::SUCCESS
        }
        Ok(Reply::RunRequest { request }) => {
            print!("{}", run_detail(&request));
            ExitCode::SUCCESS
        }
        Ok(Reply::Guide { guide }) => {
            print!("{}", guide_detail(&guide));
            ExitCode::SUCCESS
        }
        other => crate::agent::failed(&socket, other),
    }
}

/// `run  id  state  session  command`, tab-separated; the command's first shown line.
fn run_line(run: &RunRequestView) -> String {
    let state = if run.waiting_for_input {
        "waiting-for-input"
    } else {
        crate::ask::state_name(run.state)
    };
    let first_line = run.shown_command.text.lines().next().unwrap_or("");
    format!("run\t{}\t{state}\t{}\t{first_line}", run.id, run.session)
}

fn guide_line(guide: &GuideView) -> String {
    format!(
        "guide\t{}\tstep {} of {}\t{}\t{}",
        guide.id,
        guide.current_step + 1,
        guide.steps.len(),
        guide.session,
        guide.title.text
    )
}

/// Every warning the display rules raised about `shown`, as one line or none.
fn warnings(shown: &ShownText) -> String {
    let mut notes = Vec::new();
    if shown.has_hidden_characters {
        notes.push("hidden characters shown escaped".to_owned());
    }
    if !shown.non_ascii.is_empty() {
        notes.push(format!("non-ASCII: {}", shown.non_ascii.join(", ")));
    }
    if notes.is_empty() {
        String::new()
    } else {
        format!("  ! {}\n", notes.join("; "))
    }
}

fn run_detail(run: &RunRequestView) -> String {
    let mut text = format!(
        "request {} revision {} {} (session {})\n",
        run.id,
        run.revision,
        crate::ask::state_name(run.state),
        run.session
    );
    if run.asked_command.is_some() {
        text.push_str("  ! edited by the owner since the agent asked\n");
    }
    let _ = writeln!(text, "command:\n{}", run.shown_command.text);
    text.push_str(&warnings(&run.shown_command));
    let _ = writeln!(text, "directory: {}", run.shown_cwd.text);
    text.push_str(&warnings(&run.shown_cwd));
    match (&run.project, run.outside_project) {
        (Some(project), false) => {
            let _ = writeln!(text, "  inside project {project}");
        }
        (Some(project), true) => {
            let _ = writeln!(text, "  ! OUTSIDE the agent's project {project}");
        }
        (None, _) => text.push_str("  ! the agent's project is unknown\n"),
    }
    let _ = writeln!(text, "reason: {}", run.shown_reason.text);
    text.push_str(&warnings(&run.shown_reason));
    let _ = writeln!(text, "digest: {}", run.digest);
    if run.waiting_for_input {
        let _ = writeln!(
            text,
            "waiting for input: cockpit approval input {} TEXT",
            run.id
        );
    }
    if let Some(result) = &run.result {
        let _ = writeln!(
            text,
            "exit {} ({} earlier lines and {} bytes not shown, {} secrets redacted)\n{}",
            result.exit_code,
            result.omitted_lines,
            result.truncated_bytes,
            result.redacted,
            result.output
        );
    }
    if let Some(note) = &run.note {
        let _ = writeln!(text, "note: {note}");
    }
    if run.state == cockpit_protocol::RunStateView::Asking {
        let _ = writeln!(
            text,
            "approve: cockpit approval approve {} --revision {} --digest {}",
            run.id, run.revision, run.digest
        );
    }
    text
}

fn guide_detail(guide: &GuideView) -> String {
    let state = match guide.state {
        GuideStateView::Asking => "asking",
        GuideStateView::Expired => "expired",
        GuideStateView::Done => "done",
        GuideStateView::Stuck => "stuck",
        GuideStateView::Closed => "closed",
    };
    let mut text = format!(
        "guide {} {state} (session {}): {}\n",
        guide.id, guide.session, guide.title.text
    );
    text.push_str(&warnings(&guide.title));
    let _ = writeln!(text, "directory: {}", guide.cwd);
    let mut section = None;
    for (index, step) in guide.steps.iter().enumerate() {
        let step_section = step.section.as_ref().map(|shown| shown.text.as_str());
        if step_section != section {
            if let Some(heading) = step_section {
                let _ = writeln!(text, "## {heading}");
            }
            section = step_section;
        }
        let marker = if index == guide.current_step && guide.state == GuideStateView::Asking {
            ">"
        } else {
            " "
        };
        let _ = writeln!(text, "{marker} {}. {}", index + 1, step.text.text);
        text.push_str(&warnings(&step.text));
        let lines = [
            ("open", step.open.iter().collect::<Vec<_>>()),
            ("copy", step.copy.iter().collect()),
            ("ask", step.ask.iter().collect()),
            ("run", step.run.iter().collect()),
        ];
        for (key, values) in lines {
            for value in values {
                let _ = writeln!(text, "     {key}: {}", value.text);
                text.push_str(&warnings(value));
            }
        }
        if let Some(answer) = &step.answer {
            let _ = writeln!(text, "     answered: {answer}");
        }
        if let Some(request) = step.run_request {
            let _ = writeln!(text, "     filed run request {request}");
        }
    }
    if let Some(note) = &guide.stuck_note {
        let _ = writeln!(text, "stuck: {note}");
    }
    text
}
