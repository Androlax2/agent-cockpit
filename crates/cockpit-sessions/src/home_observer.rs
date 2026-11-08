use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use crate::Session;

/// Why a home is about to stop describing its account's work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HomeChange {
    /// The session ended: the home is removed after this call.
    Removal,
    /// The session relaunches on another account and keeps its home.
    AccountSwitch,
}

/// Told when a sandboxed session's own home folder is about to stop describing its account's
/// work, so the cost archive can read the home one last time (#699): the home, with the
/// conversations the provider kept in it, is removed when the session ends, and a relaunch on
/// another account starts attributing the home's next replies to that account.
///
/// Called with the session's operation lock held, so no launch or stop of the session runs
/// meanwhile. An observer is best effort and bounds its own time: nothing it does can refuse the
/// end or the relaunch.
pub trait HomeObserver: Send + Sync {
    /// `session` is as its latest launch recorded it: `session.launch`'s account is the one
    /// the home's replies so far belong to. `home` exists on disk.
    fn leaving<'observed>(
        &'observed self,
        session: &'observed Session,
        home: &'observed Path,
        change: HomeChange,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'observed>>;
}
