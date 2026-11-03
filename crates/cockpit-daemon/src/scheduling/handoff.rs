//! Owner handoff checks the selected launch and task snapshot in the placement transaction.
use super::*;

pub(super) async fn handoff(
    context: &Context,
    item: i64,
    intent_revision: i64,
    state_revision: i64,
    session: &str,
    expected_launch: i64,
) -> Answer {
    let session = cockpit_sessions::SessionId::parse(session)?;
    let write = context.database.write().await?;
    let current = cockpit_sessions::session(&write, &session).await?;
    if current.launch.as_ref().map(|launch| launch.number) != Some(expected_launch) {
        return Err(Failed(ProtocolError::new(
            ErrorCode::StaleRevision,
            "the selected session launched again; inspect it before handing off",
        )));
    }
    let inputs = inputs_in(context, &write).await?;
    let request = cockpit_scheduling::Handoff {
        item: cockpit_tasks::ItemId(item),
        intent_revision,
        state_revision,
        session: cockpit_authorization::SessionId::new(session.as_str())
            .map_err(|e| internal(&e))?,
    };
    cockpit_scheduling::handoff(
        &write,
        &request,
        &inputs,
        &context.settings,
        &Policy::for_budget(context.admission.capacity()),
        &context.admission.support(),
    )
    .await
    .map_err(|error| {
        let code = match &error {
            cockpit_scheduling::Error::StaleTask => ErrorCode::StaleRevision,
            cockpit_scheduling::Error::HandoffRefused { .. } => ErrorCode::Refused,
            cockpit_scheduling::Error::Tasks(cockpit_tasks::Error::Refused(
                cockpit_tasks::Refusal::NotFound { .. },
            )) => ErrorCode::NotFound,
            _ => return internal(&error),
        };
        Failed(ProtocolError::new(code, error.to_string()))
    })?;
    write.commit().await?;
    // As in a routine pass, committed placement survives a delayed or uncertain delivery.
    if let Err(error) = deliver(context).await {
        tracing::error!(%error, "delivering handed-off work failed");
    }
    let read = context.database.read().await?;
    let task = cockpit_tasks::item(
        &read,
        &cockpit_tasks::Actor::Owner,
        cockpit_tasks::ItemId(item),
    )
    .await
    .map_err(|error| internal(&error))?;
    Ok(Reply::Task {
        task: Box::new(crate::tasks::task_view(task)),
    })
}
