//! The cost archive's worker (#699, report 18): reads each provider's model replies past what
//! it already counted, in the owner's home and in each live session's own home, and archives
//! them through `cockpit-usage` to the account that paid for them. A reply is counted once: a
//! home's cursors move in the transaction that archives its replies, so a restart reads on from
//! where the last transaction ended.
//!
//! A sandboxed session's home is removed when the session ends, with the transcripts in it, and
//! a relaunch on another account starts a new account's replies in the same home: the sessions
//! tell [`Archivist`] before either, so the home is read one last time under its account
//! ([`HomeObserver`]). `usage-cost` answers the archive to the owner.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use cockpit_protocol::{CostArchiveView, CostRowView, ErrorCode, Outcome, ProtocolError, Reply};
use cockpit_provider_api::{Capability, Provider, ProviderError, ReplyScan, SourceCursor};
use cockpit_sessions::{HomeChange, HomeObserver, Session, Sessions};
use cockpit_turso::Database;
use cockpit_usage::{AccountName, Day, DayCost, Recorded};

const OWNER_HOME: &str = "owner";
/// How long one home's scan may take before the pass moves on: the records are the agent's to
/// write, and a scan that never returns must not stop the archive.
const SCAN_DEADLINE: Duration = Duration::from_secs(30);
/// The most days `usage-cost` answers, and the most the protocol accepts.
pub(crate) const MOST_DAYS: u32 = 366;

/// How the daemon scans for costs: the owner's home, where every harness keeps the records of
/// sessions that run unsandboxed, and how often each home is read.
#[derive(Debug, Clone)]
pub struct CostScanning {
    pub home: PathBuf,
    pub interval: Duration,
}

/// Reads homes and archives what they hold. One scan runs at a time: two reads of one home
/// would count its replies twice before the first moved the cursors.
pub(crate) struct Archivist {
    database: Arc<Database>,
    providers: Vec<Arc<dyn Provider>>,
    owner_home: PathBuf,
    one_scan: tokio::sync::Mutex<()>,
}

/// One folder holding records: the owner's home, or a session's own.
struct Home {
    provider: Arc<dyn Provider>,
    account: AccountName,
    /// Names the home in the cursors: two homes hold records of the same relative name.
    scope: String,
    path: PathBuf,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum After {
    /// The home stays, and is read again.
    Scan,
    /// The home is going: its cursors go with it.
    Removal,
}

impl Archivist {
    /// `providers` are those that report `cost`; the others are left out.
    pub(crate) fn new(
        database: Arc<Database>,
        providers: Vec<Arc<dyn Provider>>,
        scanning: &CostScanning,
    ) -> Archivist {
        Archivist {
            database,
            providers: providers
                .into_iter()
                .filter(|provider| provider.capabilities().supports(Capability::Cost))
                .collect(),
            owner_home: scanning.home.clone(),
            one_scan: tokio::sync::Mutex::new(()),
        }
    }

    /// Prices what an earlier release could not, then reads every home each `interval`.
    pub(crate) async fn scan_forever(
        self: Arc<Archivist>,
        sessions: Arc<Sessions>,
        scanning: CostScanning,
    ) {
        if self.providers.is_empty() {
            std::future::pending::<()>().await;
        }
        self.price_what_is_priceable().await;
        loop {
            self.pass(&sessions).await;
            tokio::time::sleep(scanning.interval).await;
        }
    }

    async fn price_what_is_priceable(&self) {
        let result = async {
            let write = self
                .database
                .write()
                .await
                .map_err(|error| error.to_string())?;
            let priced = cockpit_usage::price_unpriced(&write)
                .await
                .map_err(|error| error.to_string())?;
            write.commit().await.map_err(|error| error.to_string())?;
            Ok::<usize, String>(priced)
        }
        .await;
        match result {
            Ok(0) => {}
            Ok(priced) => {
                tracing::info!(priced, "priced archived replies the price table now knows")
            }
            Err(error) => tracing::error!(error, "pricing the archived replies failed"),
        }
    }

    async fn pass(&self, sessions: &Sessions) {
        let live = match sessions.list(false).await {
            Ok(live) => live,
            Err(error) => {
                tracing::error!(%error, "listing the sessions for the cost archive failed");
                return;
            }
        };
        let owner_account = AccountName::new(cockpit_config::DEFAULT_ACCOUNT)
            .expect("`default` is a valid account name");
        for provider in &self.providers {
            let owner = Home {
                provider: Arc::clone(provider),
                account: owner_account.clone(),
                scope: OWNER_HOME.to_owned(),
                path: self.owner_home.clone(),
            };
            self.scan_logged(&owner, After::Scan).await;
            for session in live
                .iter()
                .filter(|session| &session.provider == provider.name())
            {
                let Some(home) = self.session_home(sessions, provider, session) else {
                    continue;
                };
                self.scan_logged(&home, After::Scan).await;
            }
        }
    }

    /// The home of a sandboxed `session` as its latest launch's account owns it; `None` for a
    /// session with no home or no account: an unsandboxed agent works in the owner's home.
    fn session_home(
        &self,
        sessions: &Sessions,
        provider: &Arc<dyn Provider>,
        session: &Session,
    ) -> Option<Home> {
        let path = sessions
            .sandbox()
            .session_home(session.id.as_str())?
            .ok()
            .filter(|path| path.is_dir())?;
        let account = session.launch.as_ref()?.account.as_deref()?;
        let account = match AccountName::new(account) {
            Ok(account) => account,
            Err(error) => {
                tracing::warn!(session = %session.id, %error, "a session runs on an account the cost archive cannot name");
                return None;
            }
        };
        Some(Home {
            provider: Arc::clone(provider),
            account,
            scope: format!("session-{}", session.id),
            path,
        })
    }

    async fn scan_logged(&self, home: &Home, after: After) {
        let _one_at_a_time = self.one_scan.lock().await;
        match self.scan(home, after).await {
            Ok(recorded) if recorded == Recorded::default() => {}
            Ok(recorded) => tracing::info!(
                provider = %home.provider.name(),
                account = %home.account,
                scope = %home.scope,
                added = recorded.added,
                raised = recorded.raised,
                unpriced = recorded.unpriced,
                rejected = recorded.rejected,
                "archived model replies"
            ),
            Err(error) => tracing::error!(
                provider = %home.provider.name(),
                scope = %home.scope,
                error,
                "scanning for model replies failed"
            ),
        }
    }

    async fn scan(&self, home: &Home, after: After) -> Result<Recorded, String> {
        let name = home.provider.name();
        let cursors = {
            let read = self
                .database
                .read()
                .await
                .map_err(|error| error.to_string())?;
            cockpit_usage::cursors(&read, name, &home.scope)
                .await
                .map_err(|error| error.to_string())?
        };
        let scan = read_home(home, cursors).await?;
        for problem in &scan.problems {
            tracing::warn!(provider = %name, scope = %home.scope, problem, "a record source could not be read");
        }
        if scan.replies.is_empty() && scan.cursors.is_empty() && after == After::Scan {
            return Ok(Recorded::default());
        }
        let projects = self.project_folders().await?;
        let project_of = |working_dir: Option<&str>| project_of(&projects, working_dir);
        let write = self
            .database
            .write()
            .await
            .map_err(|error| error.to_string())?;
        let recorded = cockpit_usage::record_replies(
            &write,
            name,
            &home.account,
            &home.scope,
            &scan,
            &project_of,
        )
        .await
        .map_err(|error| error.to_string())?;
        if after == After::Removal {
            cockpit_usage::forget_home(&write, name, &home.scope)
                .await
                .map_err(|error| error.to_string())?;
        }
        write.commit().await.map_err(|error| error.to_string())?;
        Ok(recorded)
    }

    /// Every checkout's folder with its project, the longest folder first, so a clone inside
    /// another checkout's folder is the clone's project.
    async fn project_folders(&self) -> Result<Vec<(PathBuf, String)>, String> {
        let read = self
            .database
            .read()
            .await
            .map_err(|error| error.to_string())?;
        let mut folders: Vec<(PathBuf, String)> = cockpit_projects::checkouts(&read, None)
            .await
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(|checkout| (checkout.path, checkout.project.as_str().to_owned()))
            .collect();
        folders.sort_by_key(|(path, _)| std::cmp::Reverse(path.as_os_str().len()));
        Ok(folders)
    }
}

impl HomeObserver for Archivist {
    fn leaving<'observed>(
        &'observed self,
        session: &'observed Session,
        home: &'observed Path,
        change: HomeChange,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'observed>> {
        Box::pin(async move {
            let Some(provider) = self
                .providers
                .iter()
                .find(|provider| provider.name() == &session.provider)
            else {
                return;
            };
            let Some(Ok(account)) = session
                .launch
                .as_ref()
                .and_then(|launch| launch.account.as_deref())
                .map(AccountName::new)
            else {
                return;
            };
            let leaving = Home {
                provider: Arc::clone(provider),
                account,
                scope: format!("session-{}", session.id),
                path: home.to_path_buf(),
            };
            let after = match change {
                HomeChange::Removal => After::Removal,
                HomeChange::AccountSwitch => After::Scan,
            };
            self.scan_logged(&leaving, after).await;
        })
    }
}

async fn read_home(home: &Home, cursors: Vec<SourceCursor>) -> Result<ReplyScan, String> {
    let provider = Arc::clone(&home.provider);
    let path = home.path.clone();
    let reading = tokio::task::spawn_blocking(move || provider.scan_replies(&path, &cursors));
    match tokio::time::timeout(SCAN_DEADLINE, reading).await {
        Ok(Ok(Ok(scan))) => Ok(scan),
        Ok(Ok(Err(ProviderError { reason }))) => Err(reason),
        Ok(Err(error)) => Err(format!("the scan panicked: {error}")),
        Err(_) => Err(format!(
            "the scan of {} took more than {} s",
            home.path.display(),
            SCAN_DEADLINE.as_secs()
        )),
    }
}

/// The project a reply belongs to: the checkout folder its agent ran in, else the name of the
/// folder it ran in.
fn project_of(folders: &[(PathBuf, String)], working_dir: Option<&str>) -> String {
    let in_checkout = working_dir.and_then(|working_dir| {
        folders
            .iter()
            .find(|(folder, _)| Path::new(working_dir).starts_with(folder))
            .map(|(_, project)| project.clone())
    });
    in_checkout
        .or_else(|| {
            working_dir
                .and_then(|working_dir| Path::new(working_dir).file_name())
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "unknown".to_owned())
}

/// `usage-cost`: the archive's totals for the last `days` UTC days, today included.
pub(crate) async fn report(database: Arc<Database>, days: u32) -> Outcome {
    if !(1..=MOST_DAYS).contains(&days) {
        return Outcome::Error(ProtocolError {
            code: ErrorCode::InvalidArgument,
            message: format!("days: {days} is not 1 to {MOST_DAYS}"),
            unblock: None,
        });
    }
    let last = Day::containing(crate::approvals::now());
    let first = last.minus_days(days - 1);
    let costs = async {
        let read = database.read().await.map_err(|error| error.to_string())?;
        cockpit_usage::costs(&read, first, last)
            .await
            .map_err(|error| error.to_string())
    }
    .await;
    match costs {
        Ok(rows) => Outcome::Ok(Reply::UsageCost {
            cost: CostArchiveView {
                first_day: first.to_string(),
                last_day: last.to_string(),
                rows: rows.into_iter().map(row_view).collect(),
            },
        }),
        Err(error) => {
            tracing::error!(%error, "reading the cost archive failed");
            Outcome::Error(ProtocolError {
                code: ErrorCode::Internal,
                message: "the daemon failed; no cost was read".to_owned(),
                unblock: None,
            })
        }
    }
}

fn row_view(row: DayCost) -> CostRowView {
    CostRowView {
        day: row.day.to_string(),
        provider: row.provider.as_str().to_owned(),
        account: row.account.as_str().to_owned(),
        project: row.project,
        replies: row.replies,
        unpriced_replies: row.unpriced_replies,
        input_tokens: row.input,
        output_tokens: row.output,
        cache_read_tokens: row.cache_read,
        cache_write_tokens: row.cache_write,
        cost_nanos: row.cost_nanos,
    }
}
