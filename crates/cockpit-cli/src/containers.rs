//! `torlek containers record PROJECT`: the owner records the engine that answers as the
//! project's own rootless engine (#390), on the host-only socket. Recording again replaces it,
//! which is how a reinstalled engine is accepted.

use std::ffi::OsString;
use std::io::IsTerminal;
use std::process::ExitCode;

use cockpit_protocol::{Command, DecidedThroughView, NetworkGrantView, NetworkTargetView, Reply};

use crate::daemon::{CONTROL_DEADLINE, request, unexpected};

/// The hosts the daemon grants for `network grant PROJECT docker-hub`
/// (`cockpit_sandbox::DOCKER_HUB_HOSTS`), on 443.
const DOCKER_HUB_HOSTS: [&str; 3] = [
    "registry-1.docker.io",
    "auth.docker.io",
    "production.cloudflare.docker.com",
];

/// The command a `containers` verb sends, or the usage problem that stops it.
pub(crate) fn parse(arguments: &[OsString]) -> Result<Command, String> {
    let words = crate::ask::utf8(arguments)?;
    match words.as_slice() {
        ["record", project] => Ok(Command::ContainersRecord {
            project: (*project).to_owned(),
            through: Some(DecidedThroughView::Cli),
        }),
        _ => Err("unknown containers arguments".to_owned()),
    }
}

/// Sends `command` on the caller's socket and prints the answer; refused inside a session, so
/// no agent picks the engine its project uses.
pub(crate) fn run(command: Command) -> ExitCode {
    let socket = match crate::agent::caller_socket() {
        Ok(socket) => socket,
        Err(code) => return code,
    };
    match crate::agent::exchange(&socket, command) {
        Ok(Reply::ContainerEngineRecorded {
            project,
            user,
            engine_id,
            replaced,
        }) => {
            outln!("{project}: recorded engine {engine_id}, run by {user}");
            if let Some(replaced) = replaced {
                outln!("it replaces engine {replaced}");
            }
            offer_docker_hub_grant(&project)
        }
        other => crate::agent::failed(&socket, other),
    }
}

/// #390, decided 2026-11-09 (B): the engine's setup asks once whether the project's engine
/// may pull from Docker Hub, a revocable grant per host. Asked on a terminal, where declining
/// is the default; elsewhere only named, so a script never grants. The engine is recorded
/// either way, so only a grant the owner said yes to and that failed exits 1.
fn offer_docker_hub_grant(project: &str) -> ExitCode {
    let not_offered = || {
        eprintln!("{project}'s engine is recorded; Docker Hub was not offered");
        ExitCode::SUCCESS
    };
    let Ok(reply) = request(
        Command::NetworkGrants {
            project: Some(project.to_owned()),
        },
        CONTROL_DEADLINE,
    ) else {
        return not_offered();
    };
    let Reply::NetworkGrants { grants } = &reply else {
        unexpected(&reply);
        return not_offered();
    };
    if holds_docker_hub(grants, project) {
        return ExitCode::SUCCESS;
    }
    let offer = docker_hub_offer(&crate::profile::command(), project);
    let is_interactive = std::io::stdin().is_terminal() && std::io::stderr().is_terminal();
    if !is_interactive {
        eprintln!("{offer}");
        return ExitCode::SUCCESS;
    }
    eprint!(
        "Allow Docker Hub ({}, on 443) always for {project}'s engine? Its pulls from Docker \
         Hub work at once; a registry also takes pushes made with a credential the caller \
         supplies. `{} network revoke GRANT` takes each host back. [y/N] ",
        DOCKER_HUB_HOSTS.join(", "),
        crate::profile::command()
    );
    let mut answer = String::new();
    if let Err(error) = std::io::stdin().read_line(&mut answer) {
        eprintln!("no answer read ({error}): not granted. {offer}");
        return ExitCode::SUCCESS;
    }
    if !crate::projects::is_yes(&answer) {
        eprintln!("not granted. {offer}");
        return ExitCode::SUCCESS;
    }
    crate::network::run(crate::network::docker_hub_grant_command(project))
}

/// Whether `project` holds an active grant of each of Docker Hub's pull hosts among `grants`.
fn holds_docker_hub(grants: &[NetworkGrantView], project: &str) -> bool {
    DOCKER_HUB_HOSTS.iter().all(|docker_hub| {
        grants.iter().any(|grant| {
            let is_host = matches!(
                &grant.allows,
                NetworkTargetView::Host { host, port } if host == docker_hub && *port == 443
            );
            grant.project == project && is_host && grant.revoked.is_none()
        })
    })
}

/// The offer `containers record` makes to a project without Docker Hub's grants, naming
/// `command`, this profile's.
fn docker_hub_offer(command: &str, project: &str) -> String {
    format!(
        "{project}: its engine pulls from Docker Hub only after you allow each of its hosts' \
         requests; `{command} network grant {project} {}` allows them always",
        crate::network::DOCKER_HUB
    )
}

#[cfg(test)]
mod tests {
    use cockpit_protocol::NetworkDecisionView;

    use super::*;

    fn words(text: &str) -> Vec<OsString> {
        text.split_whitespace().map(OsString::from).collect()
    }

    #[test]
    fn record_names_the_project_and_this_client() {
        assert_eq!(
            parse(&words("record shop")),
            Ok(Command::ContainersRecord {
                project: "shop".to_owned(),
                through: Some(DecidedThroughView::Cli),
            })
        );
    }

    fn grant(project: &str, host: &str, is_revoked: bool) -> NetworkGrantView {
        let decided = NetworkDecisionView {
            at: 1,
            through: None,
        };
        NetworkGrantView {
            id: 1,
            project: project.to_owned(),
            allows: NetworkTargetView::Host {
                host: host.to_owned(),
                port: 443,
            },
            target: format!("{host}:443"),
            request: None,
            granted: decided,
            revoked: is_revoked.then_some(decided),
        }
    }

    #[test]
    fn a_project_holding_every_docker_hub_host_is_offered_nothing() {
        let grants: Vec<_> = DOCKER_HUB_HOSTS
            .iter()
            .map(|host| grant("shop", host, false))
            .collect();

        assert!(holds_docker_hub(&grants, "shop"));
    }

    #[test]
    fn a_project_missing_one_docker_hub_host_is_offered_the_grant() {
        let mut grants: Vec<_> = DOCKER_HUB_HOSTS
            .iter()
            .map(|host| grant("shop", host, false))
            .collect();
        grants[1] = grant("shop", DOCKER_HUB_HOSTS[1], true);
        let others: Vec<_> = DOCKER_HUB_HOSTS
            .iter()
            .map(|host| grant("other", host, false))
            .collect();

        assert!(!holds_docker_hub(&grants, "shop"));
        assert!(!holds_docker_hub(&others, "shop"));
    }

    #[test]
    fn the_docker_hub_offer_names_the_grant_that_answers_it() {
        let offer = docker_hub_offer("torlek", "shop");

        assert!(offer.contains("network grant shop docker-hub"), "{offer}");
    }

    #[test]
    fn anything_else_is_a_usage_problem() {
        assert!(parse(&words("record")).is_err());
        assert!(parse(&words("record shop extra")).is_err());
        assert!(parse(&words("list")).is_err());
    }
}
