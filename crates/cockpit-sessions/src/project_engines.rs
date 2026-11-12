use std::future::Future;
use std::path::Path;
use std::pin::Pin;

/// What [`ProjectEngines::reachable`] answers.
pub type EngineAnswer<'asked> = Pin<
    Box<dyn Future<Output = Result<Option<cockpit_sandbox::EngineSocket>, String>> + Send + 'asked>,
>;

/// Finds the project engine a sandboxed session reaches at `DOCKER_HOST` (#390). The daemon
/// registers it: it owns the engines' records and verifies each engine before naming it.
pub trait ProjectEngines: Send + Sync {
    /// The engine a session of `project` working in `checkout` reaches: None when it reaches
    /// none, such as an engine that is not verified or a checkout the engine does not see
    /// masked; an error refuses the launch.
    fn reachable<'asked>(
        &'asked self,
        project: &'asked str,
        checkout: &'asked Path,
    ) -> EngineAnswer<'asked>;
}
