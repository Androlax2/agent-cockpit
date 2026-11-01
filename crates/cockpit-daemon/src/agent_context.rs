//! Serves `cockpit-agent-context` (#49): the instruction bundle of the release this daemon
//! runs, to the owner and agents alike, and each agent's launch brief. The brief's facts come
//! from their owners here (the agent channel identifies the sender, sessions name its
//! workspace, workflows its step, authorization its grants); the crate only words them.

use std::sync::Arc;

use cockpit_agent_context::{Grants, LaunchFacts, WorkflowFacts};
use cockpit_protocol::{
    Command, ContextBudgetView, ErrorCode, InstructionTopicView, Outcome, ProtocolError, Reply,
    Unblock, UnblockedBy,
};
use cockpit_turso::Database;
use cockpit_turso::sql::Connection;
use cockpit_workflows::{Actor, Refusal, RunFilter, StepStatus};

use cockpit_agent_channel::{ProcessError, ProcessIdentity};

use crate::approvals::{Answer, Failed, Sender};

/// Where the daemon writes the bundle, under its state directory.
pub(crate) const BUNDLE_DIR: &str = "agent-context";

/// The folder Claude Code launches add with `--add-dir`, under the state directory: the
/// bundle is written again in its `.claude/skills`, where Claude Code looks for skills (#419).
/// A copy, not a link: Claude Code's documentation does not say it follows one.
pub(crate) const CLAUDE_SKILLS_DIR: &str = "claude-skills";

/// `instruction-catalog` and `instructions-read`: answered at once from the binary, needing
/// neither the database nor the sender's identity.
pub(crate) fn instructions(command: Command) -> Outcome {
    let manifest = cockpit_agent_context::manifest().id;
    let version = cockpit_agent_context::VERSION.to_owned();
    match command {
        Command::InstructionCatalog => Outcome::Ok(Reply::InstructionCatalog {
            version,
            manifest,
            topics: cockpit_agent_context::catalog()
                .into_iter()
                .map(|entry| InstructionTopicView {
                    name: entry.name.to_owned(),
                    summary: entry.summary.to_owned(),
                })
                .collect(),
        }),
        Command::InstructionsRead { topic: None } => Outcome::Ok(Reply::Instructions {
            version,
            manifest,
            text: cockpit_agent_context::manual(),
        }),
        Command::InstructionsRead { topic: Some(topic) } => {
            match cockpit_agent_context::document(&topic) {
                Some(document) => Outcome::Ok(Reply::Instructions {
                    version,
                    manifest,
                    text: document.body.to_owned(),
                }),
                None => Outcome::Error(ProtocolError {
                    code: ErrorCode::NotFound,
                    message: format!(
                        "no instruction topic {}",
                        topic.chars().take(64).collect::<String>()
                    ),
                    unblock: Some(Unblock {
                        action: format!(
                            "read one of {}",
                            cockpit_agent_context::catalog()
                                .iter()
                                .map(|entry| entry.name)
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                        by: UnblockedBy::Agent,
                    }),
                }),
            }
        }
        other => unreachable!("routes.rs sends only instruction reads here, not {other:?}"),
    }
}

/// `agent-brief`: the sender's launch brief from what its owners say now.
pub(crate) async fn brief(
    profile: String,
    launch_token: Option<String>,
    connector: Result<ProcessIdentity, ProcessError>,
    database: Arc<Database>,
    sessions: Arc<cockpit_sessions::Sessions>,
) -> Answer {
    let sender = Sender::read(connector).await;
    let session = identify(&sender, &database, launch_token).await?;
    let read = database.read().await?;
    let facts = facts(&read, &sessions, session, profile).await?;
    let brief = cockpit_agent_context::brief(&facts);
    let budget = cockpit_agent_context::context_budget(&brief);
    Ok(Reply::AgentBrief {
        brief,
        manifest: facts.manifest_id,
        budget: ContextBudgetView {
            brief_bytes: budget.brief_bytes as u64,
            catalog_bytes: budget.catalog_bytes as u64,
            on_demand_bytes: budget.on_demand_bytes as u64,
        },
    })
}

/// The sender's session; a launch not bound yet is bound under the writer.
async fn identify(
    sender: &Sender,
    database: &Database,
    launch_token: Option<String>,
) -> Result<cockpit_authorization::SessionId, Failed> {
    {
        let read = database.read().await?;
        if let Some(session) = sender
            .identify_bound(&read, launch_token.as_deref())
            .await?
        {
            return Ok(session);
        }
    }
    let write = database.write().await?;
    let session = sender.identify(&write, launch_token).await?;
    write.commit().await?;
    Ok(session)
}

async fn facts(
    read: &Connection,
    sessions: &cockpit_sessions::Sessions,
    session: cockpit_authorization::SessionId,
    profile: String,
) -> Result<LaunchFacts, Failed> {
    let id = cockpit_sessions::SessionId::parse(session.as_str())?;
    let working_dir = sessions.get(&id).await?.working_dir;
    // The session's own directory, not the caller's: `cockpit brief` may run from anywhere.
    let project = crate::approvals::project_at(read, &working_dir)
        .await?
        .map(|project| project.name);
    let grants = grants(read, &session).await?;
    let actor = Actor::Session {
        session: session.clone(),
        project: project.clone(),
    };
    let filter = RunFilter {
        project: project.clone(),
        include_closed: false,
    };
    // A session that may not read workflows still gets its brief, which shows it no grant.
    let runs = match cockpit_workflows::runs(read, &actor, &filter).await {
        Err(cockpit_workflows::Error::Refused(Refusal::NotAuthorized(_))) => Vec::new(),
        listed => listed?,
    };
    let workflow = runs.iter().find_map(|run| {
        let step = run.current_step()?;
        let is_doing = step.status == StepStatus::Active && step.session.as_ref() == Some(&session);
        is_doing.then(|| WorkflowFacts {
            run: run.id.0,
            item: run.item.0,
            step: step.definition.name.to_owned(),
            attempt: step.attempts,
            has_checkpoint: step.checkpoint.is_some(),
        })
    });
    Ok(LaunchFacts {
        session: session.as_str().to_owned(),
        project,
        workspace_roots: vec![working_dir],
        grants,
        workflow,
        manifest_id: cockpit_agent_context::manifest().id,
        profile,
    })
}

async fn grants(
    read: &Connection,
    session: &cockpit_authorization::SessionId,
) -> Result<Grants, Failed> {
    let revision = cockpit_authorization::current_revision(read)
        .await
        .map_err(|error| internal(&error))?;
    let held = cockpit_authorization::active_grants(read, session)
        .await
        .map_err(|error| internal(&error))?
        .into_iter()
        .map(|grant| {
            let scope = match grant.scope {
                cockpit_authorization::Scope::Owner => "everything".to_owned(),
                cockpit_authorization::Scope::Session(held) if held == *session => {
                    "this session".to_owned()
                }
                cockpit_authorization::Scope::Session(other) => {
                    format!("session {}", other.as_str())
                }
                cockpit_authorization::Scope::Task(task) => format!("task {}", task.as_str()),
            };
            format!("{} over {scope}", grant.permission.as_str())
        })
        .collect();
    Ok(Grants {
        revision: revision.value(),
        held,
    })
}

fn internal(error: &dyn std::fmt::Display) -> Failed {
    tracing::error!(%error, "an agent-brief request failed");
    Failed(ProtocolError::new(
        ErrorCode::Internal,
        "the daemon failed; the request may or may not have taken effect",
    ))
}
