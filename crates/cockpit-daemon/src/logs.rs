//! Owner-only control of the subscriber that startup installed. No state or log reads.
use std::time::UNIX_EPOCH;

use cockpit_diagnostics::{LevelControl, LevelOverride};
use cockpit_protocol::{Command, ErrorCode, Outcome, Reply};

use crate::routes::{Route, WorkClass, refusal};

pub(crate) fn route(command: Command, control: Option<LevelControl>) -> Route {
    Route::Run {
        class: WorkClass::Control,
        job: Box::pin(async move {
            tokio::task::spawn_blocking(move || apply(command, control))
                .await
                .unwrap_or_else(|_| refusal(ErrorCode::Internal, "log-level control failed"))
        }),
    }
}

fn apply(command: Command, control: Option<LevelControl>) -> Outcome {
    let Some(control) = control else {
        return refusal(
            ErrorCode::Internal,
            "the daemon did not attach its log-level control",
        );
    };
    let result = match command {
        Command::LogsLevelSet {
            directives,
            duration,
        } => {
            let requested = match LevelOverride::parse(&directives, duration.as_deref()) {
                Ok(requested) => requested,
                Err(_) => {
                    return refusal(
                        ErrorCode::InvalidArgument,
                        "use valid log directives and a positive Ns or Nm duration, at most 15m",
                    );
                }
            };
            control.set_override(requested).map(|_| ())
        }
        Command::LogsLevelRevert => control.revert(),
        Command::LogsLevelStatus => Ok(()),
        _ => unreachable!("only log-level commands reach this handler"),
    };
    if let Err(error) = result {
        return refusal(ErrorCode::Internal, &error.to_string());
    }
    let status = control.status();
    let expiry = status.active_override.map(|active| {
        active
            .expires_at
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|duration| u64::try_from(duration.as_millis()).ok())
    });
    let expires_at_unix_ms = match expiry {
        Some(None) => {
            return refusal(
                ErrorCode::Internal,
                "the log override expiry cannot be represented",
            );
        }
        Some(Some(expiry)) => Some(expiry),
        None => None,
    };
    Outcome::Ok(Reply::LogsLevel {
        directives: status.directives.to_string(),
        expires_at_unix_ms,
    })
}
