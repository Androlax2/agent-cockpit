//! The owner's `[skill_pack]` (#786): whether the optional skill pack is on, each skill on or
//! off by name, and the pinned commit the owner confirmed for skills that grant a harness
//! tools or hooks. Read at the daemon's start; `cockpit-agent-context` refuses a name the
//! pack does not hold.
use std::collections::BTreeMap;

use crate::{Layer, Source, Sourced};
use serde::Deserialize;
use toml::Spanned;

#[derive(Debug, Clone)]
pub struct SkillPackSettings {
    /// Off by default: no skill of the pack reaches an agent until the owner turns it on.
    pub enabled: Sourced<bool>,
    /// Over each skill's default: the starter skills are on once the pack is, the others off.
    pub skills: Sourced<BTreeMap<String, bool>>,
    /// The commit whose skills that set `allowed-tools` or `hooks` the owner read and accepts.
    pub grants_confirmed_at: Sourced<Option<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SkillPackTable {
    enabled: Option<Spanned<bool>>,
    skills: Option<Spanned<BTreeMap<String, bool>>>,
    grants_confirmed_at: Option<Spanned<String>>,
}

pub(super) fn settings(layers: &[Layer]) -> SkillPackSettings {
    let mut result = SkillPackSettings {
        enabled: Sourced::new(false, Source::ProductDefault),
        skills: Sourced::new(BTreeMap::new(), Source::ProductDefault),
        grants_confirmed_at: Sourced::new(None, Source::ProductDefault),
    };
    for layer in layers {
        let Some(table) = &layer.settings.skill_pack else {
            continue;
        };
        let table = table.get_ref();
        if let Some(value) = &table.enabled {
            result.enabled = Sourced::new(*value.get_ref(), layer.source_at(value.span()));
        }
        if let Some(value) = &table.skills {
            result.skills = Sourced::new(value.get_ref().clone(), layer.source_at(value.span()));
        }
        if let Some(value) = &table.grants_confirmed_at {
            result.grants_confirmed_at =
                Sourced::new(Some(value.get_ref().clone()), layer.source_at(value.span()));
        }
    }
    result
}
