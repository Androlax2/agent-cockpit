//! `torlek containers record PROJECT`: the owner records the engine that answers as the
//! project's own rootless engine (#390), on the host-only socket. Recording again replaces it,
//! which is how a reinstalled engine is accepted.

use std::ffi::OsString;
use std::process::ExitCode;

use cockpit_protocol::{Command, DecidedThroughView, Reply};

use crate::network::DOCKER_HUB_HOSTS;

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
/// may pull from Docker Hub, a revocable grant per host.
fn offer_docker_hub_grant(project: &str) -> ExitCode {
    let command = crate::profile::command();
    crate::network::offer_grant(crate::network::GrantOffer {
        project,
        not_offered: format!("{project}'s engine is recorded; Docker Hub was not offered"),
        holds: crate::network::holds_docker_hub,
        question: format!(
            "Allow Docker Hub ({}, on 443) always for {project}'s engine? Its pulls from Docker \
             Hub reach those hosts at once (a blob served from another host still asks); a \
             registry also takes pushes made with a credential the caller supplies. `{command} network revoke GRANT` takes each host back.",
            DOCKER_HUB_HOSTS.join(", "),
        ),
        offer: docker_hub_offer(&command, project),
        grant: crate::network::docker_hub_grant_command(project),
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
    use cockpit_protocol::{NetworkDecisionView, NetworkGrantView, NetworkTargetView};

    use super::*;
    use crate::network::holds_docker_hub;

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
