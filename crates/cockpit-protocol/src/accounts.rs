//! Wire views of login grants (#59, #858): the owner lets one project's sessions use a
//! provider's login account from its next launch, until revoked. A decision says when and
//! through which client, as a network grant's does.

use serde::{Deserialize, Serialize};

use crate::NetworkDecisionView;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginGrantView {
    pub id: i64,
    pub project: String,
    pub provider: String,
    pub account: String,
    pub granted: NetworkDecisionView,
    pub revoked: Option<NetworkDecisionView>,
}
