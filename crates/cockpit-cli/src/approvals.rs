//! `cockpit approval` and `cockpit guide`: the owner reads what agents ask and decides, on
//! the host-only socket (#34). Commands, directories and reasons are printed under the
//! display rules the daemon applied, never as the agent wrote them; steps count from 1.

use std::ffi::OsString;
use std::fmt::Write as _;
use std::process::ExitCode;

use cockpit_protocol::{
    Command, GuideStateView, GuideView, HiddenView, Reply, RunRequestView, RunStateView, ShownText,
};

/// The command a verb sends, or the usage problem that stops it.
pub(crate) fn parse(verb: &str, arguments: &[OsString]) -> Result<Command, String> {
    let words = crate::ask::utf8(arguments)?;
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
        ("approval", ["input", id]) => Command::ApprovalInput {
            request: id_of(id)?,
            text: input_line()?,
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

/// The text to type into a running command, one line read from standard input: a password
/// given as an argument would sit in the shell history and in every process listing.
fn input_line() -> Result<String, String> {
    let mut line = String::new();
    let read = std::io::stdin()
        .read_line(&mut line)
        .map_err(|error| format!("cannot read the input from standard input: {error}"))?;
    // At end of input nothing was typed; sending it would press Enter on the prompt.
    if read == 0 {
        return Err("no input on standard input; type the line, then Enter".to_owned());
    }
    let line = line.strip_suffix('\n').unwrap_or(&line);
    Ok(line.strip_suffix('\r').unwrap_or(line).to_owned())
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
    let line = format!("run\t{}\t{state}\t{}\t", run.id, run.session);
    let shown = run.shown_command.text.trim_end_matches('\n');
    // One line per run: a longer command shows its first line and counts the rest.
    let (preview, more) = match (shown.split_once('\n'), &run.folded_command) {
        (Some((first_line, rest)), _) => (first_line, format!("{} lines", rest.lines().count())),
        (None, Some(folded)) => (
            folded.preview.as_str(),
            format!("{} characters", folded.more_chars),
        ),
        (None, None) => return format!("{line}{shown}"),
    };
    format!(
        "{line}{preview} [more: {more}; cockpit approval show {}]",
        run.id
    )
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
        let kinds: Vec<&str> = shown
            .warnings
            .iter()
            .map(|kind| hidden_name(*kind))
            .collect();
        // An older daemon sends no kinds.
        if kinds.is_empty() {
            notes.push("hidden characters shown escaped".to_owned());
        } else {
            notes.push(format!(
                "hidden characters shown escaped ({})",
                kinds.join(", ")
            ));
        }
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

fn hidden_name(kind: HiddenView) -> &'static str {
    match kind {
        HiddenView::BidiControl => "direction controls",
        HiddenView::Invisible => "invisible characters",
        HiddenView::Control => "control codes",
        HiddenView::BlankLines => "blank-line padding",
        HiddenView::Unknown => "characters of a kind this client does not know",
    }
}

/// What the owner should weigh before approving, beyond the text: build files the session
/// changed that the command runs, and a `timeout` that would take the terminal from it.
fn run_hints(run: &RunRequestView) -> String {
    let mut text = String::new();
    if let Some(files) = &run.changed_build_files {
        if !files.is_empty() {
            let _ = writeln!(
                text,
                "  ! runs files changed in this checkout, not on the default branch: {}",
                files.join(", ")
            );
        }
    } else if run.state == RunStateView::Asking {
        text.push_str("  ! could not read which changed build files it runs\n");
    }
    if run.timeout_without_foreground {
        text.push_str(
            "  ! timeout without --foreground: a prompt in the command (sudo, read) stops it \
             instead of reading the terminal; ask for timeout --foreground\n",
        );
    }
    text
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
    text.push_str(&run_hints(run));
    text.push_str(&directory(
        &run.shown_cwd,
        run.project.as_deref(),
        run.outside_project,
    ));
    let _ = writeln!(text, "reason: {}", run.shown_reason.text);
    text.push_str(&warnings(&run.shown_reason));
    let _ = writeln!(text, "digest: {}", run.digest);
    if run.waiting_for_input {
        let _ = writeln!(
            text,
            "waiting for input: cockpit approval input {} (reads one line from standard input)",
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
    if run.state == RunStateView::Asking {
        let _ = writeln!(
            text,
            "approve: cockpit approval approve {} --revision {} --digest {}",
            run.id, run.revision, run.digest
        );
    }
    text
}

/// The directory a request works in, its hidden characters, and where it lies against the
/// agent's project.
fn directory(shown_cwd: &ShownText, project: Option<&str>, outside_project: bool) -> String {
    let mut text = format!("directory: {}\n", shown_cwd.text);
    text.push_str(&warnings(shown_cwd));
    match (project, outside_project) {
        (Some(project), false) => {
            let _ = writeln!(text, "  inside project {project}");
        }
        (Some(project), true) => {
            let _ = writeln!(text, "  ! OUTSIDE the agent's project {project}");
        }
        (None, _) => text.push_str("  ! the agent's project is unknown\n"),
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
    text.push_str(&directory(
        &guide.shown_cwd,
        guide.project.as_deref(),
        guide.outside_project,
    ));
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
