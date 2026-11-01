//! Effective grants: the only place that answers whether an action may happen.
//!
//! The owner gives a session a [`Grant`]: one [`Permission`] over one [`Scope`] (the owner's
//! whole state, one session, or one task). [`evaluate`] answers a request from the
//! authenticated [`Actor`] with a [`Decision`] carrying the authorization [`Revision`] it
//! read. Prose, payloads, skills, team messages and plugin manifests only ever request: none
//! of them is an input that can widen a grant (reports 11, 14 and 17).
//!
//! Grants live in the state database; callers pass a `cockpit-turso` transaction, so a
//! grant, a revocation or an evaluation joins the caller's atomic change (report 02).

mod error;
mod grants;
mod model;

pub use error::Error;
pub use grants::{
    MIGRATIONS, active_coordinator_grants, active_grant, active_grants, audit_of, current_revision,
    evaluate, grant, grant_start_sessions, latest_audit_on, latest_audits_by_target, record_audit,
    revoke, start_allowance, stored_grant,
};
pub use model::{
    Actor, Audit, AuditEntry, AuditedAction, Basis, Decision, Grant, GrantId, Outcome, Permission,
    Refusal, Revision, Scope, SessionId, StartAllowance, StartTerms, TaskId,
};
