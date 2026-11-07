//! Serves `cockpit-agent-context` (#49): the instruction bundle of the release this daemon
//! runs, to the owner and agents alike, and each agent's launch brief. The brief's facts come
//! from their owners here (the agent channel identifies the sender, sessions name its
//! workspace, workflows its step, authorization its grants); the crate only words them.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cockpit_agent_context::{
    Delivery, GithubTokenFact, Grants, InstalledSkill, LaunchFacts, PackChoice, PackListing,
    SkillSearch, SkillState, WorkflowFacts,
};
use cockpit_protocol::{
    Command, ContextBudgetView, ErrorCode, InstalledSkillView, InstructionTopicView, Outcome,
    PackDeliveryView, PackSkillStateView, PackSkillView, ProtocolError, Reply, SkillPackView,
    Unblock, UnblockedBy,
};
use cockpit_turso::Database;
use cockpit_turso::sql::Connection;
use cockpit_workflows::{Actor, Refusal, RunFilter, RunId, StepStatus};

use cockpit_agent_channel::{ProcessError, ProcessIdentity};

use crate::approvals::{Answer, Failed, Sender};

/// Where the daemon writes the bundle (`$TORLEK_INSTRUCTIONS`) under `state_dir`.
pub(crate) fn bundle_dir(state_dir: &Path) -> PathBuf {
    state_dir.join("agent-context")
}

/// The folder Claude Code launches add with `--add-dir`, under `state_dir`: the bundle is
/// written again in its `.claude/skills`, where Claude Code looks for skills (#419). A copy,
/// not a link: Claude Code's documentation does not say it follows one.
pub(crate) fn claude_skills_root(state_dir: &Path) -> PathBuf {
    state_dir.join("claude-skills")
}

/// The optional skill pack as one start delivered it (#786). A harness sees only the
/// installed skills in the folders it reads, so each folder has its own listing.
#[derive(Debug)]
pub struct DeliveredSkillPack {
    /// In Claude Code's `claude-skills` folder.
    pub claude: PackListing<'static>,
    /// In the bundle folder OpenCode's `skills.paths` names.
    pub opencode: PackListing<'static>,
    /// Every skill the owner and the trusted projects have, for the owner's listing.
    pub installed: Vec<InstalledSkill>,
}

/// Writes the optional skill pack (#786) as the owner's `[skill_pack]` chooses into Claude
/// Code's folder and OpenCode's, beside the bundle. The owner's skills and the trusted
/// projects' are found now: one of the same name wins over the pack's for each harness that
/// reads its folder, for every session until the next start. A setting the pack refuses
/// fails the start.
pub(crate) fn deliver_skill_pack(
    environment: &cockpit_config::Environment,
    claude_skills: &Path,
    opencode_skills: &Path,
) -> Result<DeliveredSkillPack, crate::StartError> {
    let config = cockpit_config::load(environment, None, &[]).map_err(crate::StartError::Config)?;
    let settings = config.skill_pack;
    let choice = PackChoice {
        enabled: settings.enabled.value,
        skills: settings.skills.value,
        grants_confirmed_at: settings.grants_confirmed_at.value,
    };
    let home = environment.home_dir().ok();
    let installed = cockpit_agent_context::discover_skills(&SkillSearch {
        home: home.as_deref(),
        project_roots: &config.trusted_projects.value,
    });
    let claude_reads: Vec<InstalledSkill> = installed
        .iter()
        .filter(|skill| cockpit_agent_context::CLAUDE_CODE_SKILL_FOLDERS.contains(&skill.folder))
        .cloned()
        .collect();
    let pack = cockpit_agent_context::skill_pack();
    let claude = cockpit_agent_context::select_pack(pack, &choice, claude_reads)
        .map_err(crate::StartError::SkillPack)?;
    let opencode = cockpit_agent_context::select_pack(pack, &choice, installed.clone())
        .map_err(crate::StartError::SkillPack)?;
    for (folder, listing) in [(claude_skills, &claude), (opencode_skills, &opencode)] {
        cockpit_agent_context::write_pack(folder, listing)
            .map_err(crate::StartError::InstructionBundle)?;
    }
    let delivered = |listing: &PackListing<'static>| {
        let names: Vec<&str> = listing.delivered().map(|skill| skill.name).collect();
        names.join(",")
    };
    tracing::info!(
        commit = pack.commit,
        enabled = choice.enabled,
        claude = %delivered(&claude),
        opencode = %delivered(&opencode),
        "wrote the skill pack"
    );
    Ok(DeliveredSkillPack {
        claude,
        opencode,
        installed,
    })
}

/// `skill-pack`: what the start delivered to each harness, beside the skills already
/// installed.
pub(crate) fn skill_pack_listing(delivered: &DeliveredSkillPack) -> Outcome {
    let display = |path: &Path| path.display().to_string();
    let state_view = |state: &SkillState| match state {
        SkillState::Delivered => PackSkillStateView::Delivered,
        SkillState::Off => PackSkillStateView::Off,
        SkillState::Shadowed { by } => PackSkillStateView::Shadowed { by: display(by) },
    };
    let pack = delivered.claude.pack;
    let harnesses = [
        ("claude", &delivered.claude),
        ("opencode", &delivered.opencode),
    ];
    Outcome::Ok(Reply::SkillPack {
        pack: Box::new(SkillPackView {
            source: pack.source.to_owned(),
            commit: pack.commit.to_owned(),
            tree: pack.tree.to_owned(),
            licence: pack.licence.spdx.to_owned(),
            enabled: delivered.claude.enabled,
            skills: pack
                .skills
                .iter()
                .enumerate()
                .map(|(index, skill)| PackSkillView {
                    name: skill.name.to_owned(),
                    deliveries: harnesses
                        .iter()
                        .map(|(harness, listing)| PackDeliveryView {
                            harness: (*harness).to_owned(),
                            state: state_view(&listing.skills[index].state),
                        })
                        .collect(),
                    is_starter: skill.is_starter,
                    is_user_invoked: skill.is_user_invoked(),
                    grants: skill.grants().into_iter().map(str::to_owned).collect(),
                })
                .collect(),
            installed: delivered
                .installed
                .iter()
                .map(|skill| InstalledSkillView {
                    name: skill.name.clone(),
                    scope: match skill.scope {
                        cockpit_agent_context::Scope::User => "user",
                        cockpit_agent_context::Scope::Project => "project",
                        cockpit_agent_context::Scope::Product => "product",
                    }
                    .to_owned(),
                    path: display(&skill.path),
                    collides: delivered.claude.collides(&skill.name),
                })
                .collect(),
        }),
    })
}

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
    environment: cockpit_config::Environment,
    launch_token: Option<String>,
    connector: Result<ProcessIdentity, ProcessError>,
    database: Arc<Database>,
    sessions: Arc<cockpit_sessions::Sessions>,
    skill_pack: Arc<DeliveredSkillPack>,
) -> Answer {
    let sender = Sender::read(connector).await;
    let session = identify(&sender, &database, launch_token).await?;
    let read = database.read().await?;
    let row = sessions
        .get(&cockpit_sessions::SessionId::parse(session.as_str())?)
        .await?;
    let provider = row.provider.clone();
    let mut facts = facts(&read, row.working_dir, session.clone(), &environment).await?;
    let packet = match &facts.workflow {
        Some(workflow) => {
            let actor = Actor::Session {
                session,
                project: facts.project.clone(),
            };
            Some(task_packet(&read, &actor, workflow.run).await)
        }
        None => None,
    };
    drop(read);
    if let Some(project) = facts.project.clone() {
        facts.github_token = github_token_fact(environment, project).await?;
    }
    let brief = cockpit_agent_context::brief(&facts);
    let budget = cockpit_agent_context::context_budget(
        &brief,
        Delivery {
            lists_skill: harness_lists_skill(provider.as_str()),
            mcp_instructions_bytes: cockpit_protocol::MCP_INSTRUCTIONS.len(),
            pack_catalog_bytes: pack_catalog_bytes(provider.as_str(), &skill_pack),
        },
    );
    Ok(Reply::AgentBrief {
        brief,
        manifest: facts.manifest_id,
        budget: ContextBudgetView {
            brief_bytes: budget.brief_bytes as u64,
            packet_bytes: packet.as_ref().map_or(0, String::len) as u64,
            catalog_bytes: budget.catalog_bytes as u64,
            mcp_instructions_bytes: budget.mcp_instructions_bytes as u64,
            pack_catalog_bytes: budget.pack_catalog_bytes as u64,
            on_demand_bytes: budget.on_demand_bytes as u64,
        },
        packet,
    })
}

/// Whether the provider's launch makes the harness list the `torlek` skill (#419): Claude Code
/// reads the added directory's skills and OpenCode its `skills.paths`; Codex gets the MCP
/// server and the hooks only. A provider not named here is counted as not listing it, so
/// `--measure` never claims a catalog entry nobody loads.
fn harness_lists_skill(provider: &str) -> bool {
    matches!(provider, "claude" | "opencode")
}

/// What the skill pack adds to every conversation of `provider` (#786): the delivered skills'
/// catalog entries where the harness lists the folder they are in, without the user-invoked
/// ones on Claude Code, which keeps their descriptions out of the model's context. OpenCode
/// lists every skill it finds.
fn pack_catalog_bytes(provider: &str, skill_pack: &DeliveredSkillPack) -> usize {
    match provider {
        "claude" => skill_pack.claude.catalog_bytes(true),
        "opencode" => skill_pack.opencode.catalog_bytes(false),
        _ => 0,
    }
}

/// The packet of the step the session does, as `cockpit workflow packet` prints it but with
/// only the last handoff and no decisions, capped (#781): what a context start must not lose.
/// A packet that cannot be read is said to be missing rather than costing the agent its brief.
async fn task_packet(read: &Connection, actor: &Actor, run: i64) -> String {
    let mut packet = match cockpit_workflows::packet(read, actor, RunId(run)).await {
        Ok(packet) => crate::workflows::packet_view(packet),
        Err(error) => {
            tracing::error!(%error, run, "the task packet for an agent brief failed");
            return cockpit_agent_context::unreadable_task_packet(run);
        }
    };
    let earlier = packet.handoffs.len().saturating_sub(1);
    packet.handoffs.drain(..earlier);
    packet.decisions.clear();
    cockpit_agent_context::task_packet(run, &packet.text())
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
    working_dir: PathBuf,
    session: cockpit_authorization::SessionId,
    environment: &cockpit_config::Environment,
) -> Result<LaunchFacts, Failed> {
    // The session's own directory, not the caller's: `cockpit brief` may run from anywhere.
    let checkout = crate::approvals::checkout_at(read, &working_dir).await?;
    let project = checkout
        .as_ref()
        .map(|checkout| checkout.project.as_str().to_owned());
    let guidance = guidance(environment, checkout.as_ref(), &working_dir).await?;
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
        manifest_id: cockpit_agent_context::session_manifest(guidance.clone()).id,
        guidance,
        profile: environment.profile.value.name().to_owned(),
        github_token: None,
    })
}

/// The user and project guidance files of the session's harness, read now (#420). Project
/// guidance is trusted only when the owner trusts the checkout; a directory in no registered
/// project is listed from `working_dir` as untrusted. A broken owner configuration fails the
/// brief, as it fails the launch: it must not pass for "no project is trusted".
async fn guidance(
    environment: &cockpit_config::Environment,
    checkout: Option<&cockpit_projects::Checkout>,
    working_dir: &Path,
) -> Result<Vec<cockpit_agent_context::ManifestEntry>, Failed> {
    let project_is_trusted = match checkout {
        Some(checkout) => {
            let trusted_roots = crate::projects::trusted_roots(environment.clone())
                .await
                .map_err(|error| internal(&error))?;
            matches!(
                cockpit_projects::trust(checkout, &trusted_roots),
                cockpit_projects::Trust::Trusted | cockpit_projects::Trust::Inherited { .. }
            )
        }
        None => false,
    };
    let home = environment.home.clone().map(PathBuf::from);
    let project_root =
        checkout.map_or_else(|| working_dir.to_path_buf(), |found| found.path.clone());
    tokio::task::spawn_blocking(move || {
        cockpit_agent_context::discover_guidance(&cockpit_agent_context::GuidanceSearch {
            home: home.as_deref(),
            project_root: &project_root,
            project_is_trusted,
        })
    })
    .await
    .map_err(|error| internal(&error))
}

/// Whether `project` has a GitHub token, read now (#754); none when the owner's home is
/// unknown.
async fn github_token_fact(
    environment: cockpit_config::Environment,
    project: String,
) -> Result<Option<GithubTokenFact>, Failed> {
    let state = tokio::task::spawn_blocking(move || {
        cockpit_config::github_token_state(&environment, &project)
    })
    .await
    .map_err(|error| internal(&error))?;
    Ok(state.map(|state| match state {
        cockpit_config::GithubTokenState::Configured { .. } => GithubTokenFact::Configured,
        cockpit_config::GithubTokenState::Missing { .. } => GithubTokenFact::Missing,
        cockpit_config::GithubTokenState::Unusable { .. } => GithubTokenFact::Unusable,
    }))
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

#[cfg(test)]
mod tests {
    use super::{DeliveredSkillPack, harness_lists_skill, pack_catalog_bytes};
    use cockpit_agent_context::{PackChoice, select_pack, skill_pack};

    /// The starter skills and `wait-what`, the one only the user invokes.
    fn listing() -> cockpit_agent_context::PackListing<'static> {
        let choice = PackChoice {
            enabled: true,
            skills: [("wait-what".to_owned(), true)].into(),
            grants_confirmed_at: None,
        };
        select_pack(skill_pack(), &choice, Vec::new()).unwrap()
    }

    fn delivered() -> DeliveredSkillPack {
        DeliveredSkillPack {
            claude: listing(),
            opencode: listing(),
            installed: Vec::new(),
        }
    }

    #[test]
    fn claude_code_is_charged_for_the_packs_model_invoked_skills_alone() {
        let listing = listing();

        assert_eq!(
            pack_catalog_bytes("claude", &delivered()),
            listing.catalog_bytes(true)
        );
        assert!(pack_catalog_bytes("claude", &delivered()) < listing.catalog_bytes(false));
    }

    #[test]
    fn opencode_is_charged_for_every_delivered_skill() {
        let listing = listing();

        assert_eq!(
            pack_catalog_bytes("opencode", &delivered()),
            listing.catalog_bytes(false)
        );
    }

    #[test]
    fn codex_given_no_skill_folder_is_charged_nothing_for_the_pack() {
        assert_eq!(pack_catalog_bytes("codex", &delivered()), 0);
    }

    #[test]
    fn only_the_harnesses_that_read_the_skills_folder_list_the_skill() {
        assert!(harness_lists_skill("claude"));
        assert!(harness_lists_skill("opencode"));
        assert!(!harness_lists_skill("codex"));
    }
}
