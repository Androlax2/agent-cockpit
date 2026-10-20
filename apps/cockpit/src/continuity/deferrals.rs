use super::*;
use crate::{reviews::Review, sessions::valid_repository};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeferralTarget {
    Question { session: String, fingerprint: String },
    PullRequest { repository: String, number: i64, head_oid: String, decision: String },
}

fn valid_oid(oid: &str) -> bool {
    [40,64].contains(&oid.len()) && oid.bytes().all(|byte| byte.is_ascii_hexdigit())
}

impl DeferralTarget {
    pub fn question(session: &Session) -> Option<Self> {
        if !["waiting", "needs-you"].contains(&session.state.as_str()) || !session.ask_to_run.is_empty() || !session.detail.is_empty() { return None; }
        let text = question_text(session)?;
        if text.contains("Do you want to proceed?") { return None; }
        Some(Self::Question { session: session.session.clone(), fingerprint: question_identity(session)? })
    }

    pub fn review(review: &Review) -> Option<Self> {
        if review.review != "REVIEW_REQUIRED" || review.draft || !valid_oid(&review.head_oid)
            || !valid_repository(&review.full_repository) || review.number <= 0 { return None; }
        Some(Self::PullRequest { repository: review.full_repository.clone(), number: review.number,
            head_oid: review.head_oid.to_lowercase(), decision: review.review.clone() })
    }

    pub fn storage_key(&self) -> String {
        match self {
            Self::Question { session, .. } => format!("question:{session}"),
            Self::PullRequest { repository, number, .. } => format!("pr:{repository}#{number}"),
        }
    }

    fn is_valid(&self) -> bool {
        match self {
            Self::Question { session, fingerprint } => agents::is_session_id(session)
                && fingerprint.strip_prefix("question:").is_some_and(|hash| hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())),
            Self::PullRequest { repository, number, head_oid, decision } => valid_repository(repository)
                && *number > 0 && valid_oid(head_oid) && decision == "REVIEW_REQUIRED",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Deferral {
    pub target: DeferralTarget,
    pub created_at: i64,
    pub expires_at: i64,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

impl State {
    pub(super) fn validate_deferrals(&self) -> Result<(), String> {
        if self.deferrals.len() > MAX_RECORDS { return Err("too many saved deferrals".into()); }
        for (key, deferral) in &self.deferrals {
            if !deferral.target.is_valid() || *key != deferral.target.storage_key() || deferral.created_at <= 0
                || deferral.created_at.checked_add(1200) != Some(deferral.expires_at) {
                return Err("invalid deferral target or timestamp".into());
            }
        }
        Ok(())
    }

    pub(super) fn refresh_question_deferrals(&mut self, sessions: &[Session], now: i64) {
        self.deferrals.retain(|_, deferral| deferral.created_at <= now && deferral.expires_at > now && match &deferral.target {
            DeferralTarget::Question { session, .. } => sessions.iter().find(|candidate| candidate.session == *session)
                .and_then(DeferralTarget::question).as_ref() == Some(&deferral.target),
            DeferralTarget::PullRequest { .. } => true,
        });
    }

    pub(super) fn refresh_pr_deferrals(&mut self, reviews: &[Review], now: i64) {
        self.deferrals.retain(|_, deferral| deferral.created_at <= now && deferral.expires_at > now && match &deferral.target {
            DeferralTarget::PullRequest { repository, number, head_oid, decision } => {
                reviews.iter().find(|review| review.full_repository == *repository && review.number == *number)
                    .is_none_or(|review| !valid_oid(&review.head_oid)
                        || (!review.draft && review.head_oid.eq_ignore_ascii_case(head_oid) && review.review == *decision))
            }
            DeferralTarget::Question { .. } => true,
        });
    }
}

impl Continuity {
    pub fn defer(&mut self, target: &DeferralTarget, now: i64) -> Result<bool, String> {
        if !target.is_valid() || now <= 0 { return Err("invalid deferral target".into()); }
        let expires_at = now.checked_add(1200).ok_or("invalid deferral timestamp")?;
        self.change(|state| {
            if let DeferralTarget::Question { session, fingerprint } = target {
                if !state.sessions.get(session).and_then(|memory| memory.question.as_ref()).is_some_and(|question| question.identity == *fingerprint) {
                    return Err("question is no longer current".into());
                }
            }
            let key = target.storage_key();
            let extra = state.deferrals.get(&key).map(|deferral| deferral.extra.clone()).unwrap_or_default();
            state.deferrals.insert(key, Deferral { target: target.clone(), created_at: now, expires_at, extra });
            Ok(())
        })
    }

    pub fn bring_back(&mut self, target: &DeferralTarget) -> Result<bool, String> {
        self.change(|state| { state.deferrals.remove(&target.storage_key()); Ok(()) })
    }

    pub fn is_deferred(&self, target: &DeferralTarget, now: i64) -> bool { self.deferred_until(target, now).is_some() }

    pub fn deferred_until(&self, target: &DeferralTarget, now: i64) -> Option<i64> {
        self.state.deferrals.get(&target.storage_key()).filter(|deferral|
            deferral.target == *target && deferral.created_at <= now && deferral.expires_at > now).map(|deferral| deferral.expires_at)
    }

    pub fn deferred_count(&self, now: i64) -> usize {
        self.state.deferrals.values().filter(|deferral| deferral.created_at <= now && deferral.expires_at > now).count()
    }

    pub fn saved_deferral(&self, key: &str) -> Option<&Deferral> { self.state.deferrals.get(key) }
}
