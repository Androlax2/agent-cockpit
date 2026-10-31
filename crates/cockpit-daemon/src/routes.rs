//! Request routing: which lane a command runs in and what carries it out.

use std::future::Future;
use std::pin::Pin;

use cockpit_protocol::{Command, ErrorCode, Outcome, ProtocolError, Reply, Welcome};

/// The lane a request waits in. Each lane has its own workers and its own bounded queue,
/// so bulk work can fill its lane without taking capacity from control (report 03).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkClass {
    /// Input, status, stop and approval: small, latency-bound requests.
    Control,
    /// Output, builds, provider calls and anything else that may take long.
    Bulk,
}

/// A request's work. It runs to completion once started: the daemon never aborts it, so the
/// lane's capacity is released only when the operation stopped (report 03). Blocking work
/// goes through [`tokio::task::spawn_blocking`] inside it and is awaited.
pub type Job = Pin<Box<dyn Future<Output = Outcome> + Send + 'static>>;

/// What the daemon does with one decoded request.
pub enum Route {
    /// Answer at once, without taking lane capacity: refusals that need no work.
    Answer(Outcome),
    /// Wait for a worker of `class`, or answer `busy` when its queue is full.
    Run { class: WorkClass, job: Job },
}

/// Turns commands into routes. Capabilities add their commands to [`Routes`]; the seam
/// exists so fixtures can stand in slow or failing work behind the real socket, lanes and
/// panic containment (#108).
pub trait Handler: Send + Sync + 'static {
    /// `welcome` is what this connection negotiated. Called on the connection's task: it
    /// must not block or wait; the work belongs in the [`Job`].
    fn route(&self, command: Command, welcome: &Welcome) -> Route;
}

/// The daemon's own commands.
#[derive(Debug, Default)]
pub struct Routes;

impl Handler for Routes {
    fn route(&self, command: Command, _welcome: &Welcome) -> Route {
        match command {
            Command::Ping => Route::Run {
                class: WorkClass::Control,
                job: Box::pin(async { Outcome::Ok(Reply::Pong) }),
            },
            // The daemon offers no change feed until messaging serves it (#41), so no
            // connection has negotiated it.
            Command::Subscribe { .. } => Route::Answer(refusal(
                ErrorCode::NotNegotiated,
                "this connection did not negotiate the change-feed capability",
            )),
            // `decode_request` refuses unknown kinds before routing; kept total on purpose.
            Command::Unknown => Route::Answer(refusal(
                ErrorCode::UnknownCommand,
                "this daemon does not know the command",
            )),
        }
    }
}

pub fn refusal(code: ErrorCode, message: &str) -> Outcome {
    Outcome::Error(ProtocolError {
        code,
        message: message.to_owned(),
    })
}
