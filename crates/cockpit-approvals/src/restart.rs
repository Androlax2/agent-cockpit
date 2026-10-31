//! What a daemon restart makes of the runs the previous daemon left behind, from what their
//! executions were observed to do (#163).

use std::time::Duration;

use cockpit_processes::{EndReason, Error as ProcessesError, ExecutionId, Phase};
use cockpit_turso::Database;

use crate::runs::{Interrupted, interrupted_runs, reconcile_run};
use crate::{Error, NoResult, RunState, Runner};

/// How often a run's execution is looked at while nothing but its end is awaited: first
/// after `WATCH_FIRST`, then twice as long each time up to `WATCH_LONGEST`. Each look at a
/// systemd scope runs `systemctl`, and a leftover may run for days.
const WATCH_FIRST: Duration = Duration::from_secs(1);
const WATCH_LONGEST: Duration = Duration::from_secs(30);

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

    /// Observes `execution` until it ended, and answers why it ended. A host query that
    /// failed (`systemctl`, `/proc`) is asked again at the next look; only the state database
    /// or the stored row failing ends the watch.
    pub async fn wait_until_ended(
        &self,
        database: &Database,
        execution: ExecutionId,
    ) -> Result<EndReason, Error> {
        let mut interval = WATCH_FIRST;
        loop {
            match self.supervisor.observe(database, execution).await {
                Ok(observed) => {
                    if let Phase::Ended(reason) = observed.phase {
                        return Ok(reason);
                    }
                }
                Err(ProcessesError::HostCommand { .. } | ProcessesError::Io { .. }) => {}
                Err(error) => return Err(error.into()),
            }
            tokio::time::sleep(interval).await;
            interval = (interval * 2).min(WATCH_LONGEST);
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

/// Why a run still running at a daemon restart has no result, once its execution ended for
/// `reason`: nobody read its terminal or reaped it since the restart. An execution that was
/// still launching may turn out never to have started the command.
pub fn ended_after_restart(reason: EndReason) -> NoResult {
    let how = match reason {
        EndReason::NeverStarted => {
            return NoResult::NotStarted(
                "the daemon restarted while the command was being launched; it never started"
                    .to_owned(),
            );
        }
        EndReason::Exited => "exited",
        EndReason::Terminated => "terminated",
        EndReason::HostRestarted => "the host restarted",
    };
    NoResult::Unobserved(format!(
        "the daemon restarted while the command ran; it ended later ({how}), its exit code and \
         output after the restart were not observed"
    ))
}
