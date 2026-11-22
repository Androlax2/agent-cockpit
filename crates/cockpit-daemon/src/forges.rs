//! The forge adapters the daemon pushes and reads through (#153, #60): GitHub alone in the
//! first release (#857 decision 4). Their declarations name the hosts a push may go to; the
//! owner's push right on a project's grant of one lets that project's checkouts push there.

use std::os::unix::fs::DirBuilderExt;
use std::path::Path;

use cockpit_forge_api::{Declaration, Forge, ForgeHost, HostUse};

/// The folder of the daemon's state that holds each adapter's own program configuration,
/// so no adapter reads the owner's (`~/.config/gh`).
const ADAPTERS_DIR: &str = "forges";

/// The adapters, ready to call. Blocking: each discovers its program's version, and one
/// whose program is missing withholds its capabilities naming why.
pub(crate) fn adapters(
    environment: &cockpit_config::Environment,
) -> Result<Vec<Box<dyn Forge>>, String> {
    let home = environment.home_dir().map_err(|error| error.to_string())?;
    let layout = cockpit_config::Layout::resolve(environment, &home);
    let config_dir = layout.state_dir.value.join(ADAPTERS_DIR).join("github");
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&config_dir)
        .map_err(|error| format!("{}: {error}", config_dir.display()))?;
    let gh = crate::clipboard::find_program(Path::new("gh")).unwrap_or_else(|| "gh".into());
    Ok(vec![Box::new(cockpit_github::GitHub::new(gh, config_dir))])
}

fn declarations() -> [Declaration; 1] {
    [cockpit_github::declaration()]
}

/// Every host some adapter declares for Git pushes, in order.
pub(crate) fn declared_push_hosts() -> Vec<ForgeHost> {
    let mut hosts: Vec<ForgeHost> = declarations()
        .iter()
        .flat_map(|declaration| declaration.uses().to_vec())
        .filter(|credential| credential.used_for == HostUse::GitPush)
        .map(|credential| credential.host)
        .collect();
    hosts.sort();
    hosts.dedup();
    hosts
}

/// Of the hosts the owner bound the push right on (`cockpit_sandbox::push_hosts`), those an
/// adapter declares for Git pushes: where the project's checkouts may push.
pub(crate) fn push_hosts(bound: &[String]) -> Vec<ForgeHost> {
    declared_push_hosts()
        .into_iter()
        .filter(|host| bound.iter().any(|bound| bound == host.as_str()))
        .collect()
}

/// `host` as the name and port a grant stores: 443 unless it names one. A [`ForgeHost`]'s
/// port is valid by construction.
pub(crate) fn name_and_port(host: &ForgeHost) -> (&str, u16) {
    match host.as_str().rsplit_once(':') {
        Some((name, port)) => (name, port.parse().expect("a forge host's port is valid")),
        None => (host.as_str(), 443),
    }
}
