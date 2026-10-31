//! What a daemon restart makes of the runs the previous daemon left behind, from what their
//! executions were observed to do (#163).

use std::time::Duration;

use cockpit_processes::{EndReason, Error as ProcessesError, ExecutionId, Phase};
use cockpit_turso::Database;

use crate::runs::{Interrupted, interrupted_runs, reconcile_run};
use crate::{Error, NoResult, RunState, Runner};

/// How often a run's execution is looked at while nothing but its end is awaited. Each look
/// at a systemd scope runs `systemctl`.
const WATCH_INTERVAL: Duration = Duration::from_secs(1);

impl Runner {
    /// Startup, after `cockpit-processes` reconciled the executions: ends each run the
    /// previous daemon left approved or running from what its execution did, and answers
    /// the state each is left in. [`RunState::Running`]: its execution still runs; the
    /// caller waits for its end with [`Runner::wait_until_ended`], then records it with
    /// [`ended_after_restart`].
    pub async fn reconcile(
        &self,
        database: &Database,
        now: i64,
    ) -> Result<Vec<(Interrupted, RunState)>, Error> {
        let interrupted = interrupted_runs(&*database.read().await?).await?;
        let mut phases = Vec::with_capacity(interrupted.len());
        for run in &interrupted {
            phases.push(match run.execution {
                Some(execution) => execution_phase(database, execution).await?,
                None => None,
            });
        }
        let write = database.write().await?;
        let mut reconciled = Vec::with_capacity(interrupted.len());
        for (run, phase) in interrupted.into_iter().zip(phases) {
            let state = reconcile_run(&write, &run, phase.as_ref(), now).await?;
            reconciled.push((run, state));
        }
        write.commit().await?;
        Ok(reconciled)
    }

    /// Observes `execution` until it ended, and answers why it ended.
    pub async fn wait_until_ended(
        &self,
        database: &Database,
        execution: ExecutionId,
    ) -> Result<EndReason, Error> {
        loop {
            if let Phase::Ended(reason) = self.supervisor.observe(database, execution).await?.phase
            {
                return Ok(reason);
            }
            tokio::time::sleep(WATCH_INTERVAL).await;
        }
    }
}

/// The phase of a run's execution; `None` when no execution has that id.
async fn execution_phase(
    database: &Database,
    execution: ExecutionId,
) -> Result<Option<Phase>, Error> {
    match cockpit_processes::execution(database, execution).await {
        Ok(found) => Ok(Some(found.phase)),
        Err(ProcessesError::UnknownExecution { .. }) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Why a run that outlived a daemon restart has no result, once its execution ended for
/// `reason`: nobody read its terminal or reaped it since the restart.
pub fn ended_after_restart(reason: EndReason) -> NoResult {
    NoResult::Unobserved(format!(
        "the daemon restarted while the command ran; it ended later ({}), its exit code and \
         output after the restart were not observed",
        match reason {
            EndReason::Exited => "exited",
            EndReason::Terminated => "terminated",
            EndReason::NeverStarted => "never started",
            EndReason::HostRestarted => "the host restarted",
        }
    ))
}
