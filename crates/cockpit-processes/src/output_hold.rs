//! Non-expiring managed reader/collector guards. A guard owns the database opener lifetime.
//! Acquiring one requires the writer; recovery checks users under the same writer.
use crate::{Error, ExecutionId, OutputMode, output_facts};
use cockpit_turso::{Database, WriteTransaction};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

type Key = (String, i64);
static USERS: OnceLock<Mutex<HashMap<Key, usize>>> = OnceLock::new();
fn users() -> &'static Mutex<HashMap<Key, usize>> {
    USERS.get_or_init(Mutex::default)
}

#[derive(Debug, Clone)]
pub struct OutputGuard(Arc<Use>);
#[derive(Debug)]
struct Use {
    database: Arc<Database>,
    key: Key,
}
impl Drop for Use {
    fn drop(&mut self) {
        let mut users = users().lock().expect("output registry not poisoned");
        let count = users.get_mut(&self.key).expect("registered output user");
        *count -= 1;
        if *count == 0 {
            users.remove(&self.key);
        }
    }
}
impl OutputGuard {
    pub fn execution(&self) -> ExecutionId {
        ExecutionId(self.0.key.1)
    }
    pub fn database(&self) -> &Arc<Database> {
        &self.0.database
    }
}

pub fn output_users(database_epoch: &str, id: ExecutionId) -> usize {
    users()
        .lock()
        .expect("output registry not poisoned")
        .get(&(database_epoch.to_owned(), id.0))
        .copied()
        .unwrap_or(0)
}

pub async fn guard_output(database: Arc<Database>, id: ExecutionId) -> Result<OutputGuard, Error> {
    let write = database.write().await?;
    let guard = guard_output_in(&write, Arc::clone(&database), id).await?;
    write.commit().await?;
    Ok(guard)
}

/// Caller keeps the writer until its consumer/generation check and guard acquisition commit.
pub async fn guard_output_in(
    write: &WriteTransaction<'_>,
    database: Arc<Database>,
    id: ExecutionId,
) -> Result<OutputGuard, Error> {
    let facts = output_facts(write, id).await?;
    if facts.origin_epoch.as_deref() != Some(database.epoch())
        || matches!(facts.mode, OutputMode::Pty | OutputMode::Hosted)
    {
        return Err(crate::output_catalog::invalid(
            id,
            "output is historical, unknown-epoch, or not captured in files",
        ));
    }
    if facts.retirement.is_some() {
        return Err(crate::output_catalog::invalid(id, "output was retired"));
    }
    let key = (database.epoch().to_owned(), id.0);
    *users()
        .lock()
        .expect("output registry not poisoned")
        .entry(key.clone())
        .or_default() += 1;
    Ok(OutputGuard(Arc::new(Use { database, key })))
}
