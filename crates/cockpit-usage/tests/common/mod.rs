//! A scratch state database with the usage tables, and readings to record in it.
#![allow(dead_code, reason = "each test binary uses a different subset")]

use std::path::PathBuf;

use cockpit_provider_api::{
    LimitKey, ModelReply, ProviderName, QuotaReading, QuotaWindow, ReplyScan, ReplyTokens,
};
use cockpit_turso::Database;
use cockpit_usage::{
    AccountName, AccountQuota, Alert, Approval, Day, DayCost, DayDigest, DayTime, Error, Incident,
    QuotaPolicy, Recorded, Turn,
};

/// 2027-01-15 08:00 UTC.
pub const NOW: i64 = 1_800_000_000 - 1_800_000_000 % 86_400 + 8 * 3600;
pub const HOUR: i64 = 3600;
/// The five-hour window's reset in most tests: two hours from `NOW`.
pub const RESET: i64 = NOW + 2 * HOUR;

pub struct Fixture {
    pub dir: tempfile::TempDir,
    pub database: Database,
    pub policy: QuotaPolicy,
}

impl Fixture {
    pub async fn new() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let database = open(dir.path().join("state.db")).await;
        Fixture {
            dir,
            database,
            policy: QuotaPolicy::default(),
        }
    }

    /// The same database file opened again, as after a daemon restart.
    pub async fn reopened(self) -> Fixture {
        let Fixture {
            dir,
            database,
            policy,
        } = self;
        drop(database);
        let database = open(dir.path().join("state.db")).await;
        Fixture {
            dir,
            database,
            policy,
        }
    }

    pub async fn record_for(
        &self,
        provider: &str,
        account: &str,
        reading: &QuotaReading,
        observed_at: i64,
    ) -> Result<Vec<Alert>, Error> {
        let write = self.database.write().await.unwrap();
        let alerts = cockpit_usage::record_reading(
            &write,
            &ProviderName::new(provider).unwrap(),
            &AccountName::new(account).unwrap(),
            reading,
            &self.policy,
            observed_at,
        )
        .await;
        write.commit().await.unwrap();
        alerts
    }

    pub async fn record(&self, reading: &QuotaReading, observed_at: i64) -> Vec<Alert> {
        self.record_for("claude", "main", reading, observed_at)
            .await
            .unwrap()
    }

    pub async fn fail(&self, reason: &str, failed_at: i64) -> Result<(), Error> {
        let write = self.database.write().await.unwrap();
        let recorded = cockpit_usage::record_failure(
            &write,
            &ProviderName::new("claude").unwrap(),
            &AccountName::new("main").unwrap(),
            reason,
            failed_at,
        )
        .await;
        write.commit().await.unwrap();
        recorded
    }

    pub async fn quotas_at(&self, now: i64) -> Vec<AccountQuota> {
        let read = self.database.read().await.unwrap();
        cockpit_usage::quotas(&read, &self.policy, now)
            .await
            .unwrap()
    }

    pub async fn only_account_at(&self, now: i64) -> AccountQuota {
        let mut quotas = self.quotas_at(now).await;
        assert_eq!(quotas.len(), 1, "{quotas:#?}");
        quotas.remove(0)
    }
}

async fn open(path: PathBuf) -> Database {
    Database::open(&path, &[cockpit_usage::MIGRATIONS])
        .await
        .unwrap()
}

pub fn window(limit: &str, used_percent: f64, resets_at: Option<i64>) -> QuotaWindow {
    QuotaWindow::new(
        LimitKey::new(limit).unwrap(),
        None,
        Some(18_000),
        used_percent,
        resets_at,
    )
    .unwrap()
}

/// A reading of the five-hour window alone.
pub fn five_hour(used_percent: f64, resets_at: i64) -> QuotaReading {
    QuotaReading::new(
        Some("max".to_owned()),
        vec![window("five-hour", used_percent, Some(resets_at))],
        false,
    )
    .unwrap()
}

pub fn thresholds(alerts: &[Alert]) -> Vec<(String, u8)> {
    alerts
        .iter()
        .map(|alert| (alert.limit.as_str().to_owned(), alert.threshold))
        .collect()
}

/// 2026-11-02 00:00 UTC.
pub const MONDAY: i64 = 1_793_577_600;
pub const DAY: i64 = 86_400;

pub fn reply(key: &str, at: i64, model: &str, tokens: ReplyTokens) -> ModelReply {
    ModelReply::new(key, at, model, Some("/work/app".to_owned()), tokens).unwrap()
}

pub fn tokens(input: u64, output: u64) -> ReplyTokens {
    ReplyTokens {
        input,
        output,
        ..ReplyTokens::default()
    }
}

pub fn scan_of(replies: Vec<ModelReply>) -> ReplyScan {
    ReplyScan {
        replies,
        ..ReplyScan::default()
    }
}

pub fn app_project(_working_dir: Option<&str>) -> String {
    "app".to_owned()
}

impl Fixture {
    pub async fn archive_for(
        &self,
        account: &str,
        home_scope: &str,
        scan: &ReplyScan,
    ) -> Result<Recorded, Error> {
        self.archive_in_project(account, home_scope, scan, &app_project)
            .await
    }

    pub async fn archive_in_project(
        &self,
        account: &str,
        home_scope: &str,
        scan: &ReplyScan,
        project_of: &(dyn Fn(Option<&str>) -> String + Sync),
    ) -> Result<Recorded, Error> {
        let write = self.database.write().await.unwrap();
        let recorded = cockpit_usage::record_replies(
            &write,
            &ProviderName::new("claude").unwrap(),
            &AccountName::new(account).unwrap(),
            home_scope,
            scan,
            project_of,
        )
        .await;
        write.commit().await.unwrap();
        recorded
    }

    pub async fn archive(&self, replies: Vec<ModelReply>) -> Recorded {
        self.archive_for("main", "owner", &scan_of(replies))
            .await
            .unwrap()
    }

    pub async fn costs_between(&self, first: &str, last: &str) -> Vec<DayCost> {
        let read = self.database.read().await.unwrap();
        cockpit_usage::costs(&read, Day::parse(first).unwrap(), Day::parse(last).unwrap())
            .await
            .unwrap()
    }

    pub async fn all_costs(&self) -> Vec<DayCost> {
        self.costs_between("2026-01-01", "2026-12-31").await
    }
}

/// `hour:minute` of the `day_offset`th day after `MONDAY`, in Unix seconds.
pub fn clock(day_offset: i64, hour: i64, minute: i64) -> i64 {
    MONDAY + day_offset * DAY + hour * HOUR + minute * 60
}

impl Fixture {
    pub async fn turn(&self, session: &str, project: &str, started_at: i64, ended_at: i64) -> bool {
        let turn = Turn::new(session, project, started_at, ended_at).unwrap();
        let write = self.database.write().await.unwrap();
        let added = cockpit_usage::record_turn(&write, &turn).await.unwrap();
        write.commit().await.unwrap();
        added
    }

    pub async fn approval(&self, key: &str, project: &str, at: i64) -> bool {
        let approval = Approval::new(key, project, at).unwrap();
        let write = self.database.write().await.unwrap();
        let added = cockpit_usage::record_approval(&write, &approval)
            .await
            .unwrap();
        write.commit().await.unwrap();
        added
    }

    pub async fn incident(
        &self,
        provider: &str,
        key: &str,
        title: &str,
        started_at: i64,
        resolved_at: Option<i64>,
    ) {
        let incident = Incident::new(
            ProviderName::new(provider).unwrap(),
            key,
            title,
            started_at,
            resolved_at,
        )
        .unwrap();
        let write = self.database.write().await.unwrap();
        cockpit_usage::record_incident(&write, &incident)
            .await
            .unwrap();
        write.commit().await.unwrap();
    }

    pub async fn time_between(&self, first: &str, last: &str) -> Result<Vec<DayTime>, Error> {
        let read = self.database.read().await.unwrap();
        cockpit_usage::time_with_agent(&read, Day::parse(first).unwrap(), Day::parse(last).unwrap())
            .await
    }

    pub async fn digests_between(&self, first: &str, last: &str) -> Vec<DayDigest> {
        let read = self.database.read().await.unwrap();
        cockpit_usage::digests(&read, Day::parse(first).unwrap(), Day::parse(last).unwrap())
            .await
            .unwrap()
    }
}
