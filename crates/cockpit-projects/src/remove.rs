//! Unregistering a project (#727): its rows stay as history and nothing on disk is touched.

use cockpit_turso::WriteTransaction;

use crate::store::{mark_project_removed, project_checkouts};
use crate::{Checkout, CheckoutKind, CheckoutState, Error, Project};

/// Marks `project` and its checkouts removed in `write`, returning its clones: they stay on
/// disk, no longer tracked, so the sweep never deletes them. Refuses while a checkout is
/// occupied, being swept or held by a storage cleanup. The caller checks the sessions and
/// task items in the same transaction.
pub async fn remove_project(
    write: &WriteTransaction<'_>,
    project: &Project,
) -> Result<Vec<Checkout>, Error> {
    let checkouts = project_checkouts(write, project.id).await?;
    for checkout in &checkouts {
        if let Some(occupancy) = &checkout.occupancy {
            return Err(Error::Occupied {
                checkout: checkout.id,
                holder: occupancy.holder.clone(),
            });
        }
        if checkout.state != CheckoutState::Live {
            return Err(Error::BeingRemoved {
                checkout: checkout.id,
            });
        }
        crate::require_checkout_available(write, checkout.id).await?;
    }
    mark_project_removed(write, project.id).await?;
    Ok(checkouts
        .into_iter()
        .filter(|checkout| checkout.kind != CheckoutKind::Main)
        .collect())
}
