//! `cockpit resources`: the shared budget, what this host enforces, the profile and the
//! three pause operations, served by the daemon.

use std::ffi::OsString;
use std::process::ExitCode;

use cockpit_protocol::{
    Command, EffectiveView, ProfileView, Reply, ReservationView, ResourceStatusView, VerdictView,
};

use crate::daemon::{BULK_DEADLINE, CONTROL_DEADLINE, request, unexpected, utf8_words};

/// A `resources` verb's command, or the usage problem that stops it.
pub fn parse(arguments: &[OsString]) -> Result<Command, String> {
    let words = utf8_words(arguments)?;
    Ok(match words.as_slice() {
        ["status"] => Command::ResourceStatus,
        ["profile", "normal"] => Command::ResourceProfile {
            profile: ProfileView::Normal,
        },
        ["profile", "foreground"] => Command::ResourceProfile {
            profile: ProfileView::Foreground,
        },
        ["pause-queue"] => Command::ResourceQueue { paused: true },
        ["resume-queue"] => Command::ResourceQueue { paused: false },
        ["check", workload, options @ ..] => check(workload, options)?,
        ["checkpoint", id] => Command::ResourceCheckpoint {
            reservation: reservation(id)?,
        },
        ["stop", id] => Command::ResourceStop {
            reservation: reservation(id)?,
        },
        _ => return Err("unknown resources arguments".to_owned()),
    })
}

/// `check WORKLOAD --cpu MILLICORES --memory MIB [--require CONTROL]...`.
fn check(workload: &str, options: &[&str]) -> Result<Command, String> {
    let (mut cpu_millis, mut memory_mib, mut required) = (None, None, Vec::new());
    let mut options = options.iter();
    while let Some(flag) = options.next() {
        let value = options
            .next()
            .ok_or_else(|| format!("{flag} needs a value"))?;
        match *flag {
            "--require" => required.push((*value).to_owned()),
            "--cpu" | "--memory" => {
                let number = value
                    .parse::<u64>()
                    .map_err(|_| format!("{flag} {value}: not a whole number"))?;
                let slot = if *flag == "--cpu" {
                    &mut cpu_millis
                } else {
                    &mut memory_mib
                };
                if slot.replace(number).is_some() {
                    return Err(format!("{flag} given twice"));
                }
            }
            _ => return Err(format!("unknown argument {flag}")),
        }
    }
    Ok(Command::ResourceCheck {
        workload: workload.to_owned(),
        cpu_millis: cpu_millis.ok_or("check needs --cpu MILLICORES")?,
        memory_mib: memory_mib.ok_or("check needs --memory MIB")?,
        required,
    })
}

fn reservation(id: &str) -> Result<i64, String> {
    id.parse()
        .map_err(|_| format!("{id}: not a reservation number"))
}

/// Sends `command` and prints what it answers. Exit 1 when the daemon refuses, when a check
/// is held, and when a stop was not yet observed to end the execution.
pub fn run(command: Command) -> ExitCode {
    let deadline = match command {
        Command::ResourceStop { .. } => BULK_DEADLINE,
        _ => CONTROL_DEADLINE,
    };
    let reply = match request(command, deadline) {
        Ok(reply) => reply,
        Err(code) => return code,
    };
    match &reply {
        Reply::ResourceStatus { status } => out!("{}", status_text(status)),
        Reply::Reservation { reservation } => out!("{}", reservation_text(reservation)),
        Reply::ResourceVerdict { verdict } => match verdict {
            VerdictView::Fits => outln!("fits"),
            VerdictView::Held { reason } => {
                outln!("held: {reason}");
                return ExitCode::FAILURE;
            }
        },
        Reply::ResourceStopped {
            reservation,
            observed_ended,
        } => {
            if !observed_ended {
                outln!(
                    "reservation {reservation}: stop requested, but its execution was not \
                     observed ended; it keeps its reservation"
                );
                return ExitCode::FAILURE;
            }
            outln!("reservation {reservation}: its execution was observed ended; released");
        }
        _ => return unexpected(&reply),
    }
    ExitCode::SUCCESS
}

/// The host and settings, one line per control, then each reservation with its limits.
fn status_text(status: &ResourceStatusView) -> String {
    let mut text = format!(
        "platform {}\tcontainment {}\tprofile {}\tqueue {}\n\
         budget {} millicores {} MiB\tin use {} millicores {} MiB\tforeground heavy share {}%\n",
        status.platform,
        status.containment,
        status.profile.as_str(),
        if status.queue_paused {
            "paused"
        } else {
            "running"
        },
        status.budget.cpu_millis,
        status.budget.memory_mib,
        status.in_use.cpu_millis,
        status.in_use.memory_mib,
        status.foreground_heavy_percent,
    );
    for control in &status.controls {
        text.push_str(&format!(
            "control {}\t{}\t{}\n",
            control.control,
            yes_no(control.supported),
            control.detail
        ));
    }
    for reservation in &status.reservations {
        text.push_str(&reservation_text(reservation));
    }
    text
}

/// `reservation ID  workload  state  execution  reserved  label`, then one indented line
/// per control: requested, supported, effective.
fn reservation_text(reservation: &ReservationView) -> String {
    let mut text = format!(
        "reservation {}\t{}\t{}{}\texecution {}\treserved {} millicores {} MiB\t{}\n",
        reservation.id,
        reservation.workload,
        reservation.state,
        if reservation.checkpoint_requested {
            " (checkpoint requested)"
        } else {
            ""
        },
        reservation
            .execution
            .map_or("-".to_owned(), |execution| execution.to_string()),
        reservation.reserved.cpu_millis,
        reservation.reserved.memory_mib,
        reservation.label,
    );
    for limit in &reservation.limits {
        let effective = match &limit.effective {
            EffectiveView::Value { value } => value.clone(),
            EffectiveView::Unknown { why } => format!("unknown ({why})"),
        };
        text.push_str(&format!(
            "  {}\trequested {}\tsupported {}\teffective {effective}\n",
            limit.control,
            limit.requested.as_deref().unwrap_or("-"),
            yes_no(limit.supported),
        ));
    }
    text
}

fn yes_no(flag: bool) -> &'static str {
    if flag { "yes" } else { "no" }
}
