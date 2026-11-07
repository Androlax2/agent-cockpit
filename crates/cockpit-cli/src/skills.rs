//! `torlek skills`: the optional skill pack the daemon delivered at its start (#786), each
//! skill's state, then the skills the owner and the trusted projects already have, a name
//! the pack also holds marked.

use std::ffi::OsString;
use std::process::ExitCode;

use cockpit_protocol::{Command, PackSkillStateView, PackSkillView, Reply, SkillPackView};

use crate::daemon::{CONTROL_DEADLINE, request, unexpected, utf8_words};

pub(crate) fn run(arguments: &[OsString]) -> Result<ExitCode, String> {
    if let Some(argument) = utf8_words(arguments)?.first() {
        return Err(format!("skills takes no argument, not {argument}"));
    }
    Ok(match request(Command::SkillPack, CONTROL_DEADLINE) {
        Ok(Reply::SkillPack { pack }) => {
            out!("{}", listing(&pack));
            ExitCode::SUCCESS
        }
        Ok(other) => unexpected(&other),
        Err(code) => code,
    })
}

fn listing(pack: &SkillPackView) -> String {
    let switch = if pack.enabled { "on" } else { "off" };
    let mut lines = vec![format!(
        "skill pack: {switch}, {} at {} (tree {}), {}",
        pack.source, pack.commit, pack.tree, pack.licence
    )];
    lines.extend(pack.skills.iter().map(skill_line));
    lines.push("installed:".to_owned());
    lines.extend(pack.installed.iter().map(|skill| {
        let collision = if skill.collides {
            "\tsame name as a pack skill"
        } else {
            ""
        };
        format!("{}\t{}\t{}{collision}", skill.name, skill.scope, skill.path)
    }));
    lines.iter().map(|line| format!("{line}\n")).collect()
}

fn skill_line(skill: &PackSkillView) -> String {
    let mut cells = vec![skill.name.clone()];
    cells.extend(skill.deliveries.iter().map(|delivery| {
        let state = match &delivery.state {
            PackSkillStateView::Delivered => "delivered".to_owned(),
            PackSkillStateView::Off => "off".to_owned(),
            PackSkillStateView::Shadowed { by } => format!("shadowed by {by}"),
        };
        format!("{}: {state}", delivery.harness)
    }));
    if skill.is_starter {
        cells.push("starter".to_owned());
    }
    if skill.is_user_invoked {
        cells.push("user-invoked".to_owned());
    }
    if !skill.grants.is_empty() {
        cells.push(format!("grants {}", skill.grants.join(", ")));
    }
    cells.join("\t")
}
