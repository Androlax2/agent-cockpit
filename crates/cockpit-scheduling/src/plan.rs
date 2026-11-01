//! One pass's decisions, without the database: which waiting work goes to which session, in
//! project-then-task order, and why the rest waits. Deterministic: the same inputs give the
//! same placements in the same order.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use cockpit_authorization::SessionId;
use cockpit_resource_admission::{Capacity, Demand, Hold, Resource};
use cockpit_tasks::ItemId;
use cockpit_workflows::StepDefinition;

use crate::{
    ActionBy, Candidate, Explanation, Presence, Reason, Role, SessionNote, Settings, Unit,
};

/// One unit of work in the queue.
#[derive(Debug, Clone)]
pub(crate) struct Waiting {
    pub(crate) item: ItemId,
    pub(crate) project: String,
    pub(crate) title: String,
    pub(crate) unit: Unit,
    /// The step's definition, whose required capabilities the session's provider must report.
    pub(crate) step: Option<&'static StepDefinition>,
    /// No step names a role until `cockpit-teams` assigns them.
    pub(crate) role: Option<Role>,
    pub(crate) priority: i64,
    pub(crate) since: i64,
}

/// What a pass decides from, besides the waiting work.
pub(crate) struct Context<'a> {
    pub(crate) settings: &'a Settings,
    pub(crate) now: i64,
    pub(crate) candidates: &'a [Candidate],
    pub(crate) viewed: Option<&'a SessionId>,
    /// Sessions that cannot take anything, with why: doing work, not authorized.
    pub(crate) unavailable: &'a HashMap<SessionId, String>,
    /// Active placements per project: a project's share counts its placed work, never its
    /// sessions, so fan-out does not raise its entitlement.
    pub(crate) placed_by_project: BTreeMap<String, u32>,
    /// Projects a placement has finished in.
    pub(crate) familiar: &'a BTreeSet<String>,
    /// What admission says of an agent turn before room: pause, controls, feasibility. One
    /// for each demand a turn can have.
    pub(crate) familiar_hold: Option<Hold>,
    pub(crate) unfamiliar_hold: Option<Hold>,
    /// The budget left after admission's reservations and the active placements.
    pub(crate) free: Demand,
}

pub(crate) struct Planned {
    pub(crate) waiting: Waiting,
    pub(crate) session: SessionId,
    pub(crate) demand: Demand,
}

#[derive(Default)]
pub(crate) struct Plan {
    pub(crate) placements: Vec<Planned>,
    pub(crate) explanations: Vec<Explanation>,
}

/// Decides every waiting unit. Projects take turns, the one with the fewest placed units
/// first; inside a project the highest aged priority goes first, then the oldest.
pub(crate) fn plan(waiting: Vec<Waiting>, mut context: Context<'_>) -> Plan {
    let mut by_project: BTreeMap<String, Vec<Waiting>> = BTreeMap::new();
    for unit in waiting {
        by_project
            .entry(unit.project.clone())
            .or_default()
            .push(unit);
    }
    for units in by_project.values_mut() {
        units.sort_by_key(|unit| {
            (
                std::cmp::Reverse(unit.priority + aging_boost(&context, unit)),
                unit.since,
                unit.item,
            )
        });
        // Taken from the end.
        units.reverse();
    }
    let mut plan = Plan::default();
    let mut taken: HashMap<SessionId, ItemId> = HashMap::new();
    let mut gathering: Option<ItemId> = None;
    while let Some(project) = next_project(&by_project, &context) {
        let units = by_project
            .get_mut(&project)
            .expect("next_project names a project");
        let unit = units.pop().expect("next_project names a project with work");
        if units.is_empty() {
            by_project.remove(&project);
        }
        match decide(&unit, &context, &taken, gathering) {
            Decision::Place { session, demand } => {
                taken.insert(session.clone(), unit.item);
                *context.placed_by_project.entry(project).or_default() += 1;
                context.free = minus(context.free, demand);
                plan.placements.push(Planned {
                    waiting: unit,
                    session,
                    demand,
                });
            }
            Decision::Wait(explanation) => {
                let explanation = *explanation;
                if explanation.reason == Reason::NoRoom
                    && gathering.is_none()
                    && context.now - unit.since >= seconds(context.settings.gather_after)
                {
                    gathering = Some(unit.item);
                }
                plan.explanations.push(explanation);
            }
        }
    }
    plan
}

/// The project whose turn it is: the fewest placed units, then the longest-waiting next
/// unit, then the name.
fn next_project(
    by_project: &BTreeMap<String, Vec<Waiting>>,
    context: &Context<'_>,
) -> Option<String> {
    by_project
        .iter()
        .filter_map(|(project, units)| Some((project, units.last()?)))
        .min_by_key(|(project, next)| {
            (
                context
                    .placed_by_project
                    .get(*project)
                    .copied()
                    .unwrap_or(0),
                next.since,
                (*project).clone(),
            )
        })
        .map(|(project, _)| project.clone())
}

enum Decision {
    Place { session: SessionId, demand: Demand },
    Wait(Box<Explanation>),
}

fn decide(
    unit: &Waiting,
    context: &Context<'_>,
    taken: &HashMap<SessionId, ItemId>,
    gathering: Option<ItemId>,
) -> Decision {
    let is_familiar = context.familiar.contains(&unit.project);
    let headroom_percent = if is_familiar {
        0
    } else {
        context.settings.startup_headroom_percent
    };
    let demand = with_headroom(context.settings.turn_demand, headroom_percent);
    let wait = |reason: Reason, why: String, action: String, action_by: ActionBy| Explanation {
        item: unit.item,
        project: unit.project.clone(),
        title: unit.title.clone(),
        unit: unit.unit.clone(),
        reason,
        why,
        limited_by: None,
        sessions: Vec::new(),
        behind: None,
        action,
        action_by,
        priority: unit.priority,
        aging_boost: aging_boost(context, unit),
        demand,
        headroom_percent,
        waiting_since: Some(unit.since),
        evaluated_at: context.now,
        next_evaluation_at: context.now + seconds(context.settings.interval),
    };
    let hold = if is_familiar {
        &context.familiar_hold
    } else {
        &context.unfamiliar_hold
    };
    if let Some(hold) = hold {
        return Decision::Wait(Box::new(held(hold, wait)));
    }
    let project_sessions: Vec<&Candidate> = context
        .candidates
        .iter()
        .filter(|candidate| candidate.project.as_deref() == Some(unit.project.as_str()))
        .collect();
    if project_sessions.is_empty() {
        return Decision::Wait(Box::new(wait(
            Reason::NoSession,
            format!("no session works in project {}", unit.project),
            format!(
                "start a session in project {}: `cockpit session new DIR`",
                unit.project
            ),
            ActionBy::Owner,
        )));
    }
    let mut notes = Vec::new();
    let mut eligible = Vec::new();
    for candidate in project_sessions {
        match refusal(candidate, unit, context, taken) {
            Some(why) => notes.push(SessionNote {
                session: candidate.session.clone(),
                why,
            }),
            None => eligible.push(candidate),
        }
    }
    let Some(session) = eligible.into_iter().min_by_key(|candidate| {
        (
            waiting_since(candidate),
            candidate.session.as_str().to_owned(),
        )
    }) else {
        let mut explanation = wait(
            Reason::NoSessionFree,
            format!(
                "none of the {} sessions of project {} can take it now",
                notes.len(),
                unit.project
            ),
            "wait: a later pass places it once a session is free".to_owned(),
            ActionBy::Cockpit,
        );
        explanation.sessions = notes;
        return Decision::Wait(Box::new(explanation));
    };
    if let Some(shortage) = shortage(demand, context.free) {
        let mut explanation = held(&shortage, wait);
        explanation.why.push_str(", after the placed agent turns");
        if gathering.is_none() && context.now - unit.since >= seconds(context.settings.gather_after)
        {
            explanation.why.push_str(
                "; it has waited long enough to gather capacity: work after it waits until it starts",
            );
        }
        return Decision::Wait(Box::new(explanation));
    }
    if let Some(first) = gathering {
        let mut explanation = wait(
            Reason::Behind,
            format!("item {} waits for room first and gathers capacity", first.0),
            format!("wait: it starts once item {} has started", first.0),
            ActionBy::Cockpit,
        );
        explanation.behind = Some(first);
        return Decision::Wait(Box::new(explanation));
    }
    Decision::Place {
        session: session.session.clone(),
        demand,
    }
}

/// An explanation of an admission hold.
fn held(
    hold: &Hold,
    wait: impl Fn(Reason, String, String, ActionBy) -> Explanation,
) -> Explanation {
    let why = hold.to_string();
    let (reason, action, action_by, limited_by) = match hold {
        Hold::QueuePaused => (
            Reason::QueuePaused,
            "resume the queue: `cockpit resources resume-queue`".to_owned(),
            ActionBy::Owner,
            None,
        ),
        Hold::ControlUnsupported { control, .. } => (
            Reason::ControlUnsupported,
            format!(
                "make this host enforce {}, or stop requiring it",
                control.as_str()
            ),
            ActionBy::Owner,
            Some(format!("{} on this host", control.as_str())),
        ),
        Hold::Infeasible {
            resource,
            limited_by,
            ..
        } => (
            Reason::Infeasible,
            "raise the budget or lower the agent-turn demand: as configured it never starts"
                .to_owned(),
            ActionBy::Owner,
            Some(limit(*resource, *limited_by)),
        ),
        Hold::NoRoom {
            resource,
            limited_by,
            ..
        } => (
            Reason::NoRoom,
            "wait: running work releases capacity, then a later pass places it".to_owned(),
            ActionBy::Cockpit,
            Some(limit(*resource, *limited_by)),
        ),
    };
    let mut explanation = wait(reason, why, action, action_by);
    explanation.limited_by = limited_by;
    explanation
}

fn limit(resource: Resource, capacity: Capacity) -> String {
    let capacity = match capacity {
        Capacity::Budget => "the shared budget",
        Capacity::ForegroundShare => "the foreground profile's heavy-job share",
    };
    format!("{} in {capacity}", resource.as_str())
}

/// Why `candidate` cannot take `unit` now; `None` when it can. What can never change for this
/// unit (role, provider) comes first, then what changes on its own.
fn refusal(
    candidate: &Candidate,
    unit: &Waiting,
    context: &Context<'_>,
    taken: &HashMap<SessionId, ItemId>,
) -> Option<String> {
    match (unit.role, candidate.role) {
        (Some(needed), held) if held != Some(needed) => {
            return Some(format!("the step needs the {} role", needed.as_str()));
        }
        (None, Some(held)) => {
            return Some(format!(
                "it holds the {} role and takes only that role's steps",
                held.as_str()
            ));
        }
        _ => {}
    }
    if let Some(step) = unit.step
        && let Err(refusal) = cockpit_workflows::check_capabilities(step, &candidate.capabilities)
    {
        return Some(refusal.to_string());
    }
    if let Some(why) = context.unavailable.get(&candidate.session) {
        return Some(why.clone());
    }
    if let Some(item) = taken.get(&candidate.session) {
        return Some(format!("it takes item {} in this pass", item.0));
    }
    let since = match &candidate.presence {
        Presence::Inactive { state } => return Some(format!("it is {state}")),
        Presence::Unreported => {
            return Some("its agent has not reported since its launch".to_owned());
        }
        Presence::Working => return Some("its agent is working".to_owned()),
        Presence::NeedsInput => return Some("it waits for your answer".to_owned()),
        Presence::Waiting { since } => *since,
    };
    if context.viewed == Some(&candidate.session) {
        return Some("you are viewing it".to_owned());
    }
    let remaining = seconds(context.settings.idle_window) - (context.now - since);
    if remaining > 0 {
        return Some(format!(
            "idle window: {remaining} s left before it gets new work"
        ));
    }
    None
}

fn waiting_since(candidate: &Candidate) -> i64 {
    match candidate.presence {
        Presence::Waiting { since } => since,
        _ => i64::MAX,
    }
}

fn aging_boost(context: &Context<'_>, unit: &Waiting) -> i64 {
    let step = seconds(context.settings.aging_step).max(1);
    ((context.now - unit.since).max(0) / step).min(i64::from(context.settings.aging_max_boost))
}

/// The first resource `demand` lacks in `free`, as admission would hold it.
fn shortage(demand: Demand, free: Demand) -> Option<Hold> {
    [
        (Resource::Cpu, demand.cpu_millis, free.cpu_millis),
        (Resource::Memory, demand.memory_mib, free.memory_mib),
    ]
    .into_iter()
    .find(|(_, needed, available)| needed > available)
    .map(|(resource, needed, available)| Hold::NoRoom {
        resource,
        demand: needed,
        available,
        limited_by: Capacity::Budget,
    })
}

pub(crate) fn with_headroom(turn: Demand, percent: u64) -> Demand {
    Demand {
        cpu_millis: turn.cpu_millis + turn.cpu_millis * percent / 100,
        memory_mib: turn.memory_mib + turn.memory_mib * percent / 100,
    }
}

pub(crate) fn minus(from: Demand, taken: Demand) -> Demand {
    Demand {
        cpu_millis: from.cpu_millis.saturating_sub(taken.cpu_millis),
        memory_mib: from.memory_mib.saturating_sub(taken.memory_mib),
    }
}

pub(crate) fn seconds(duration: std::time::Duration) -> i64 {
    i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
}
