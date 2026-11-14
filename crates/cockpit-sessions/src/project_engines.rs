use std::future::Future;
use std::path::Path;
use std::pin::Pin;

/// What [`ProjectEngines::reachable`] answers.
pub type EngineAnswer<'asked> =
    Pin<Box<dyn Future<Output = Option<cockpit_sandbox::EngineSocket>> + Send + 'asked>>;

/// Finds the project engine a sandboxed session reaches at `DOCKER_HOST` (#390). The daemon
/// registers it: it owns the engines' records and verifies each engine before naming it.
pub trait ProjectEngines: Send + Sync {
    /// The engine a session of `project` working in `checkout` reaches, or None: an engine
    /// that is not verified, or does not see the checkout masked, is reached by no launch.
    /// The owner's readiness report says why; a launch never fails for want of an engine.
    fn reachable<'asked>(
        &'asked self,
        project: &'asked str,
        checkout: &'asked Path,
    ) -> EngineAnswer<'asked>;

    /// The Docker client configuration (`config.json`) a session working in `checkout` that
    /// reaches an engine gets in its `DOCKER_CONFIG` folder: the engine's proxy for its
    /// containers and builds, and the checkout's compose services kept direct. Read at
    /// launch: a service added later goes through the proxy until the next launch. Blocks.
    fn docker_client_config(&self, checkout: &Path) -> String;
}
