//! Request routing: which lane a command runs in and what carries it out.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use cockpit_protocol::{
    Command, ErrorCode, Outcome, ProtocolError, Reply, Unblock, UnblockedBy, Welcome,
};

use cockpit_agent_channel::{ProcessError, ProcessIdentity};

use crate::{agent, projects};

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

/// Who is at the other end of a connection, as the socket it came through and the kernel
/// say; never what a payload claims (report 11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Peer {
    /// The host-only socket, which no sandbox reaches: the owner.
    Owner,
    /// The agent socket. `connector` is the process the kernel names as having connected,
    /// identified (pid and start time) when the connection was accepted, so a later process
    /// that reuses its pid is not taken for it. The agent channel decides which session, if
    /// any, that process acts for.
    Agent {
        connector: Result<ProcessIdentity, ProcessError>,
    },
}

/// Turns commands into routes. Capabilities add their commands to [`Routes`]; the seam
/// exists so fixtures can stand in slow or failing work behind the real socket, lanes and
/// panic containment (#108).
pub trait Handler: Send + Sync + 'static {
    /// `welcome` is what this connection negotiated and `peer` who is on it. Called on the
    /// connection's task: it must not block or wait; the work belongs in the [`Job`].
    fn route(&self, command: Command, welcome: &Welcome, peer: Peer) -> Route;
}

/// The daemon's commands, over the state database the daemon opened.
pub struct Routes {
    database: Arc<cockpit_turso::Database>,
    /// The host Git that project commands run, with the safe settings applied.
    git: cockpit_projects::Git,
    /// Read for `projects.trusted` on each answer, so a changed owner config applies at once.
    environment: cockpit_config::Environment,
}

impl Routes {
    pub fn new(
        database: Arc<cockpit_turso::Database>,
        git: cockpit_projects::Git,
        environment: cockpit_config::Environment,
    ) -> Routes {
        Routes {
            database,
            git,
            environment,
        }
    }
}

impl Handler for Routes {
    fn route(&self, command: Command, _welcome: &Welcome, peer: Peer) -> Route {
        match (command, peer) {
            (Command::Ping, _) => Route::Run {
                class: WorkClass::Control,
                job: Box::pin(async { Outcome::Ok(Reply::Pong) }),
            },
            // The daemon offers no change feed until messaging serves it (#41), so no
            // connection has negotiated it.
            (Command::Subscribe { .. }, _) => Route::Answer(refusal(
                ErrorCode::NotNegotiated,
                "this connection did not negotiate the change-feed capability",
            )),
            // Agent requests run in the bulk lane: sandboxed senders must never take the
            // owner's control capacity (docs/DECISIONS.md, #32).
            (
                Command::AgentReport {
                    launch_token,
                    report,
                },
                Peer::Agent { connector },
            ) => Route::Run {
                class: WorkClass::Bulk,
                job: Box::pin(agent::report(
                    Arc::clone(&self.database),
                    connector,
                    launch_token,
                    report,
                )),
            },
            (Command::AgentReport { .. }, Peer::Owner) => Route::Answer(wrong_socket(
                "agent-report is an agent's request; the owner has no session",
                "send it on the agent socket from a session Cockpit launched",
                UnblockedBy::Agent,
            )),
            (Command::AgentReports, Peer::Owner) => Route::Run {
                class: WorkClass::Control,
                job: Box::pin(agent::reports(Arc::clone(&self.database))),
            },
            (Command::AgentReports, Peer::Agent { .. }) => Route::Answer(owner_only()),
            (
                command @ (Command::ProjectAdd { .. }
                | Command::ProjectList
                | Command::CheckoutList { .. }
                | Command::CheckoutClone { .. }
                | Command::CheckoutReview { .. }
                | Command::CheckoutClaim { .. }
                | Command::CheckoutRelease { .. }
                | Command::CheckoutSweep { .. }
                | Command::CheckoutPush { .. }),
                peer,
            ) => match peer {
                // A sandboxed agent must not register a project, take a checkout or push.
                Peer::Agent { .. } => Route::Answer(owner_only()),
                Peer::Owner => projects::route(
                    command,
                    Arc::clone(&self.database),
                    &self.git,
                    &self.environment,
                ),
            },
            // `decode_request` refuses unknown kinds before routing; kept total on purpose.
            (Command::Unknown, _) => Route::Answer(refusal(
                ErrorCode::UnknownCommand,
                "this daemon does not know the command",
            )),
        }
    }
}

pub fn refusal(code: ErrorCode, message: &str) -> Outcome {
    Outcome::Error(ProtocolError::new(code, message))
}

/// The answer to an owner's command sent on the agent socket.
fn owner_only() -> Outcome {
    wrong_socket(
        "this command is the owner's alone",
        "ask the owner to run it on the host",
        UnblockedBy::Owner,
    )
}

/// The answer to a command sent on the socket of the party it is not for.
fn wrong_socket(message: &str, action: &str, by: UnblockedBy) -> Outcome {
    Outcome::Error(ProtocolError {
        code: ErrorCode::NotAuthorized,
        message: message.to_owned(),
        unblock: Some(Unblock {
            action: action.to_owned(),
            by,
        }),
    })
}
