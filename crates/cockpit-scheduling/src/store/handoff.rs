//! Explicit owner placement. The caller authenticates the owner and checks the selected
//! session launch in this same transaction; task changes, admission and outbox stay here.
use super::*;

pub struct Handoff {
    pub item: ItemId,
    pub intent_revision: i64,
    pub state_revision: i64,
    pub session: SessionId,
}

pub async fn handoff(
    write: &WriteTransaction<'_>,
    request: &Handoff,
    inputs: &Inputs,
    settings: &Settings,
    policy: &Policy,
    support: &[ControlSupport],
) -> Result<Placement, Error> {
    let refuse = |reason: &str| Error::HandoffRefused {
        reason: reason.to_owned(),
    };
    let mut item = cockpit_tasks::item(write, &Actor::Owner, request.item).await?;
    if item.intent_revision != request.intent_revision
        || item.state_revision != request.state_revision
    {
        return Err(Error::StaleTask);
    }
    let items = cockpit_tasks::list(write, &Actor::Owner, &ListFilter::default()).await?;
    let runs = cockpit_workflows::runs(write, &Actor::Owner, &RunFilter::default()).await?;
    if runs.iter().any(|run| run.item == item.id) {
        return Err(refuse(
            "this item has a running workflow; use its workflow view",
        ));
    }
    let active = placements(write).await?;
    if active
        .iter()
        .any(|p| p.item == item.id || p.session == request.session)
    {
        return Err(refuse(
            "the item or session already has a placement; wait for it to settle",
        ));
    }
    // Selecting an item is the owner's explicit opt-in, without changing its queue flag
    // if eligibility or admission refuses the placement.
    item.is_queued = true;
    // Handing it over is the owner's answer to a spent budget, so none holds it (#412).
    let (waiting, outside) = units(std::slice::from_ref(&item), &[], &BTreeMap::new());
    if let Some(outside) = outside.into_iter().next() {
        return Err(Error::HandoffRefused {
            reason: outside.why,
        });
    }
    if waiting.is_empty() {
        return Err(refuse("only a todo item can be handed to a session"));
    }
    let candidates: Vec<_> = idle_since(write, &inputs.candidates)
        .await?
        .into_iter()
        .filter(|c| c.session == request.session)
        .collect();
    let unavailable = unavailable(write, &items, &runs, inputs).await?;
    let familiar = familiar_projects(write).await?;
    let unreserved =
        active
            .iter()
            .filter(|p| p.reservation.is_none())
            .fold(Demand::default(), |mut sum, p| {
                sum.cpu_millis += p.demand.cpu_millis;
                sum.memory_mib += p.demand.memory_mib;
                sum
            });
    let in_use = cockpit_resource_admission::in_use(write, inputs.now).await?;
    let turn = settings.turn_demand;
    let unfamiliar = plan::with_headroom(turn, settings.startup_headroom_percent);
    let mut plan = plan::plan(
        waiting,
        Context {
            settings,
            now: inputs.now,
            candidates: &candidates,
            viewed: inputs.viewed.as_ref(),
            unavailable: &unavailable,
            placed_by_project: BTreeMap::new(),
            familiar: &familiar,
            familiar_hold: admission_hold(write, turn, policy, support, inputs.now).await?,
            unfamiliar_hold: admission_hold(write, unfamiliar, policy, support, inputs.now).await?,
            free: plan::minus(plan::minus(policy.budget, in_use.all), unreserved),
            // The owner hands this item over: pacing holds back the queue, not the owner.
            usage: &[],
        },
    );
    match plan.placements.pop() {
        Some(planned) => place(write, planned, &candidates, policy, support, inputs.now).await,
        None => {
            let reason = plan
                .explanations
                .into_iter()
                .map(|e| {
                    let notes = e
                        .sessions
                        .into_iter()
                        .map(|s| s.why)
                        .collect::<Vec<_>>()
                        .join("; ");
                    format!("{}; {} {}", e.why, notes, e.action)
                })
                .collect::<Vec<_>>()
                .join("; ");
            Err(Error::HandoffRefused { reason })
        }
    }
}
