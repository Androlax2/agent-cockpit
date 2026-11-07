//! The optional skill pack (#786): skills of another author, vendored in `pack/` from one
//! upstream commit with their licence and never edited, which the owner turns on as a pack
//! and then on or off by name. The daemon writes the delivered ones into the folders the
//! adapters already list skills from. Prose grants nothing; a skill whose frontmatter grants
//! tools or hooks to a harness is refused until the owner confirms that pin.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::manifest::{Scope, hex};
use crate::write::{WriteError, ensure_dir, io, replace};

/// Beside the delivered skills in each folder: the licence and the pack's manifest.
pub const PACK_DIR: &str = "skill-pack";

/// The pack manifest's file, in [`PACK_DIR`].
const PACK_MANIFEST_FILE: &str = "manifest.json";

/// The licence's file, in [`PACK_DIR`].
const LICENCE_FILE: &str = "LICENSE";

/// Frontmatter keys with which a skill grants a harness something when it runs: Claude Code's
/// `allowed-tools` pre-approves tools, `hooks` runs commands.
const GRANTING_KEYS: [&str; 2] = ["allowed-tools", "hooks"];

/// Upstream skills pinned to one commit, the owner's to turn on.
#[derive(Debug)]
pub struct Pack {
    pub source: &'static str,
    /// The pinned commit, in full.
    pub commit: &'static str,
    /// Git's id of the commit's root tree.
    pub tree: &'static str,
    pub licence: Licence,
    pub skills: Vec<PackSkill>,
}

#[derive(Debug)]
pub struct Licence {
    pub spdx: &'static str,
    pub text: &'static str,
}

#[derive(Debug)]
pub struct PackSkill {
    /// Its directory's name, upstream and where it is delivered.
    pub name: &'static str,
    /// Its directory in the upstream tree.
    pub upstream_path: &'static str,
    /// Git's id of that directory's tree at the pinned commit.
    pub tree: &'static str,
    /// On once the owner turns the pack on, unless the owner turns it off by name.
    pub is_starter: bool,
    /// Every file of the directory, `SKILL.md` among them.
    pub files: Vec<PackFile>,
}

#[derive(Debug)]
pub struct PackFile {
    /// Relative to the skill's directory, `/`-separated.
    pub path: &'static str,
    pub text: &'static str,
}

impl PackSkill {
    /// `SKILL.md`'s YAML frontmatter without its `---` fences: what a harness lists, measured
    /// as the `torlek` skill's catalog entry is. Empty for a file without frontmatter.
    pub fn frontmatter(&self) -> &'static str {
        self.files
            .iter()
            .find(|file| file.path == "SKILL.md")
            .and_then(|file| file.text.strip_prefix("---\n"))
            .and_then(|rest| rest.split_once("\n---\n"))
            .map_or("", |(frontmatter, _)| frontmatter)
    }

    /// The keys among `allowed-tools` and `hooks` its frontmatter sets.
    pub fn grants(&self) -> Vec<&'static str> {
        GRANTING_KEYS
            .into_iter()
            .filter(|key| self.top_level_keys().any(|set| set == *key))
            .collect()
    }

    /// Whether only the user invokes it (`disable-model-invocation: true`): Claude Code then
    /// keeps its description out of the model's context.
    pub fn is_user_invoked(&self) -> bool {
        self.frontmatter()
            .lines()
            .any(|line| line.trim_end() == "disable-model-invocation: true")
    }

    fn top_level_keys(&self) -> impl Iterator<Item = &'static str> {
        self.frontmatter()
            .lines()
            .filter(|line| !line.starts_with([' ', '\t', '#']))
            .filter_map(|line| line.split_once(':'))
            .map(|(key, _)| key.trim().trim_matches(['"', '\'']))
    }
}

macro_rules! vendored {
    ($skill:literal: $($path:literal),+ $(,)?) => {
        vec![$(PackFile {
            path: $path,
            text: include_str!(concat!("../pack/", $skill, "/", $path)),
        }),+]
    };
}

static PACK: LazyLock<Pack> = LazyLock::new(|| Pack {
    source: "https://github.com/mattpocock/skills",
    commit: "4ae2317ddb22b046e26455e4f9c9b3aaaf62212d",
    tree: "a73b182b5f22565e829e0969eb0bfd284508a7b8",
    licence: Licence {
        spdx: "MIT",
        text: include_str!("../pack/LICENSE"),
    },
    skills: vec![
        PackSkill {
            name: "diagnosing-bugs",
            upstream_path: "skills/engineering/diagnosing-bugs",
            tree: "5fdd6cc53385395441910b026c72d71768e993d4",
            is_starter: true,
            files: vendored!("diagnosing-bugs":
                "SKILL.md", "agents/openai.yaml", "scripts/hitl-loop.template.sh"),
        },
        PackSkill {
            name: "tdd",
            upstream_path: "skills/engineering/tdd",
            tree: "667dbdd00c09d0e2189e082986e6a4d61a67c2a6",
            is_starter: true,
            files: vendored!("tdd": "SKILL.md", "agents/openai.yaml", "mocking.md", "tests.md"),
        },
        PackSkill {
            name: "writing-for-agents",
            upstream_path: "skills/productivity/writing-for-agents",
            tree: "ad2925850efb8973a72d2e666f7a975f9a2d4a9b",
            is_starter: true,
            files: vendored!("writing-for-agents":
                "SKILL-MECHANICS.md", "SKILL.md", "agents/openai.yaml"),
        },
        PackSkill {
            name: "pr",
            upstream_path: "skills/engineering/pr",
            tree: "00b896d811adffad747c7ca30078d909d60c7f93",
            is_starter: false,
            files: vendored!("pr": "CREDITS.md", "SKILL.md", "agents/openai.yaml"),
        },
        PackSkill {
            name: "codebase-design",
            upstream_path: "skills/engineering/codebase-design",
            tree: "4cbe1c131e1ea54109cb1e4e5f3c505810db6606",
            is_starter: false,
            files: vendored!("codebase-design":
                "DEEPENING.md", "DESIGN-IT-TWICE.md", "SKILL.md", "agents/openai.yaml"),
        },
        PackSkill {
            name: "wait-what",
            upstream_path: "skills/productivity/wait-what",
            tree: "c8b050e6b8c711be1a9008ceacb2126914f0763e",
            is_starter: false,
            files: vendored!("wait-what": "SKILL.md", "agents/openai.yaml"),
        },
    ],
});

/// The pack this release vendors.
pub fn skill_pack() -> &'static Pack {
    &PACK
}

/// The owner's `[skill_pack]` settings. The default is off.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PackChoice {
    /// Off, no skill of the pack is delivered whatever `skills` says.
    pub enabled: bool,
    /// A skill on or off by name, over its starter default.
    pub skills: BTreeMap<String, bool>,
    /// The commit at which the owner confirmed the skills that grant tools or hooks.
    pub grants_confirmed_at: Option<String>,
}

/// A skill the owner or a project already has, in a folder a harness lists skills from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledSkill {
    pub name: String,
    /// [`Scope::User`] for the owner's, [`Scope::Project`] for a project's.
    pub scope: Scope,
    /// The skill folder it is in, relative to the home or the project: which harness reads
    /// it.
    pub folder: &'static str,
    /// Its directory.
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillState {
    Delivered,
    Off,
    /// An installed skill of the same name wins: the pack's is not delivered.
    Shadowed {
        by: PathBuf,
    },
}

impl SkillState {
    pub fn as_str(&self) -> &'static str {
        match self {
            SkillState::Delivered => "delivered",
            SkillState::Off => "off",
            SkillState::Shadowed { .. } => "shadowed",
        }
    }
}

#[derive(Debug)]
pub struct ListedSkill<'pack> {
    pub skill: &'pack PackSkill,
    pub state: SkillState,
}

/// What the owner's choice delivers, beside the skills already installed.
#[derive(Debug)]
pub struct PackListing<'pack> {
    pub pack: &'pack Pack,
    pub enabled: bool,
    /// Every skill of the pack, in its order.
    pub skills: Vec<ListedSkill<'pack>>,
    pub installed: Vec<InstalledSkill>,
}

impl<'pack> PackListing<'pack> {
    pub fn delivered(&self) -> impl Iterator<Item = &'pack PackSkill> + '_ {
        self.skills
            .iter()
            .filter(|listed| listed.state == SkillState::Delivered)
            .map(|listed| listed.skill)
    }

    /// Whether an installed skill has the name of one of the pack's.
    pub fn collides(&self, installed_name: &str) -> bool {
        self.pack
            .skills
            .iter()
            .any(|skill| skill.name == installed_name)
    }

    /// The bytes the delivered skills add to every conversation of a harness that lists the
    /// folder: their frontmatter, without the user-invoked ones when the harness hides them.
    pub fn catalog_bytes(&self, hides_user_invoked: bool) -> usize {
        self.delivered()
            .filter(|skill| !(hides_user_invoked && skill.is_user_invoked()))
            .map(|skill| skill.frontmatter().len())
            .sum()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackError {
    UnknownSkill {
        name: String,
        known: Vec<&'static str>,
    },
    /// Skills that would be delivered set `allowed-tools` or `hooks` and the owner has not
    /// confirmed this commit: each skill with the keys it sets.
    UnconfirmedGrants {
        skills: Vec<(String, Vec<&'static str>)>,
        commit: &'static str,
    },
}

impl fmt::Display for PackError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PackError::UnknownSkill { name, known } => write!(
                formatter,
                "`skill_pack.skills` names {name:?}, which the skill pack does not hold; it \
                 holds {}",
                known.join(", ")
            ),
            PackError::UnconfirmedGrants { skills, commit } => {
                let named: Vec<String> = skills
                    .iter()
                    .map(|(name, keys)| format!("{name} ({})", keys.join(", ")))
                    .collect();
                write!(
                    formatter,
                    "the skill pack's {} grant a harness tools or hooks at commit {commit}; \
                     read them, then set `skill_pack.grants_confirmed_at = \"{commit}\"` to \
                     deliver them, or turn them off by name",
                    named.join(", ")
                )
            }
        }
    }
}

impl std::error::Error for PackError {}

/// What `choice` delivers of `pack`, an `installed` skill of the same name winning. A name
/// the pack does not hold is refused, and so is a delivered skill that grants tools or hooks
/// before the owner confirms the pin.
pub fn select_pack<'pack>(
    pack: &'pack Pack,
    choice: &PackChoice,
    installed: Vec<InstalledSkill>,
) -> Result<PackListing<'pack>, PackError> {
    if let Some(unknown) = choice
        .skills
        .keys()
        .find(|name| pack.skills.iter().all(|skill| skill.name != name.as_str()))
    {
        return Err(PackError::UnknownSkill {
            name: unknown.clone(),
            known: pack.skills.iter().map(|skill| skill.name).collect(),
        });
    }
    let skills: Vec<ListedSkill<'pack>> = pack
        .skills
        .iter()
        .map(|skill| {
            let is_on = choice.enabled
                && choice
                    .skills
                    .get(skill.name)
                    .copied()
                    .unwrap_or(skill.is_starter);
            let shadow = installed
                .iter()
                .find(|installed_skill| installed_skill.name == skill.name);
            let state = match (is_on, shadow) {
                (false, _) => SkillState::Off,
                (true, Some(shadow)) => SkillState::Shadowed {
                    by: shadow.path.clone(),
                },
                (true, None) => SkillState::Delivered,
            };
            ListedSkill { skill, state }
        })
        .collect();
    let is_confirmed = choice.grants_confirmed_at.as_deref() == Some(pack.commit);
    let unconfirmed: Vec<(String, Vec<&'static str>)> = skills
        .iter()
        .filter(|listed| listed.state == SkillState::Delivered)
        .map(|listed| (listed.skill.name.to_owned(), listed.skill.grants()))
        .filter(|(_, keys)| !keys.is_empty())
        .collect();
    if !is_confirmed && !unconfirmed.is_empty() {
        return Err(PackError::UnconfirmedGrants {
            skills: unconfirmed,
            commit: pack.commit,
        });
    }
    Ok(PackListing {
        pack,
        enabled: choice.enabled,
        skills,
        installed,
    })
}

/// Where the owner's and the projects' own skills are looked for.
#[derive(Debug, Clone, Copy)]
pub struct SkillSearch<'a> {
    /// None when the daemon cannot tell, which finds no owner skill.
    pub home: Option<&'a Path>,
    /// The projects whose skills a harness loads: the ones the owner trusts.
    pub project_roots: &'a [PathBuf],
}

/// The skill folders of each harness, under the owner's home: Claude Code's, the shared
/// `.agents` one Codex and OpenCode read, and OpenCode's own.
const USER_SKILL_FOLDERS: [&str; 3] = [
    ".claude/skills",
    ".agents/skills",
    ".config/opencode/skills",
];

/// The folders above Claude Code lists skills from, under the home and under a project
/// (code.claude.com/docs/en/skills); OpenCode lists every one (opencode.ai/docs/skills). An
/// installed skill shadows the pack's only for a harness that reads its folder.
pub const CLAUDE_CODE_SKILL_FOLDERS: [&str; 1] = [".claude/skills"];

/// The same, under a project root.
const PROJECT_SKILL_FOLDERS: [&str; 3] = [".claude/skills", ".agents/skills", ".opencode/skills"];

/// Every directory holding a `SKILL.md` in those folders, the owner's first, then each
/// project's. A folder that cannot be read lists nothing: it holds no skill a harness loads.
pub fn discover_skills(search: &SkillSearch<'_>) -> Vec<InstalledSkill> {
    let user = search
        .home
        .into_iter()
        .flat_map(|home| USER_SKILL_FOLDERS.map(|folder| (Scope::User, home, folder)));
    let projects = search.project_roots.iter().flat_map(|root| {
        PROJECT_SKILL_FOLDERS.map(|folder| (Scope::Project, root.as_path(), folder))
    });
    user.chain(projects)
        .flat_map(|(scope, base, folder)| skills_in(base, folder, scope))
        .collect()
}

fn skills_in(base: &Path, folder: &'static str, scope: Scope) -> Vec<InstalledSkill> {
    let Ok(entries) = fs::read_dir(base.join(folder)) else {
        return Vec::new();
    };
    let mut skills: Vec<InstalledSkill> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.join("SKILL.md").is_file())
        .filter_map(|path| {
            let name = path.file_name()?.to_str()?.to_owned();
            Some(InstalledSkill {
                name,
                scope,
                folder,
                path,
            })
        })
        .collect();
    skills.sort_by(|left, right| left.name.cmp(&right.name));
    skills
}

#[derive(Serialize)]
struct PackManifest<'listing> {
    source: &'static str,
    commit: &'static str,
    tree: &'static str,
    licence: ManifestLicence,
    skills: Vec<ManifestSkill<'listing>>,
}

#[derive(Serialize)]
struct ManifestLicence {
    spdx: &'static str,
    file: &'static str,
    sha256: String,
}

#[derive(Serialize)]
struct ManifestSkill<'listing> {
    name: &'static str,
    upstream_path: &'static str,
    tree: &'static str,
    state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    shadowed_by: Option<&'listing Path>,
    files: Vec<ManifestFile>,
}

#[derive(Serialize)]
struct ManifestFile {
    path: &'static str,
    sha256: String,
}

fn sha256(text: &str) -> String {
    hex(&Sha256::digest(text.as_bytes()))
}

/// Writes the delivered skills of `listing` under `dir`, byte for byte, and [`PACK_DIR`] with
/// the licence and the manifest beside them; removes every other skill of the pack, and the
/// whole of it when nothing is delivered. Nothing else under `dir` is touched: `dir` is a
/// folder of Torlek's state the adapters list, never the owner's.
pub fn write_pack(dir: &Path, listing: &PackListing<'_>) -> Result<(), WriteError> {
    for dropped in dropped_since_last_write(dir, listing.pack) {
        remove(&dir.join(dropped))?;
    }
    for listed in &listing.skills {
        let skill_dir = dir.join(listed.skill.name);
        match listed.state {
            SkillState::Delivered => write_skill(dir, &skill_dir, listed.skill)?,
            SkillState::Off | SkillState::Shadowed { .. } => remove(&skill_dir)?,
        }
    }
    let pack_dir = dir.join(PACK_DIR);
    if listing.delivered().next().is_none() {
        return remove(&pack_dir);
    }
    ensure_dir(dir, &pack_dir)?;
    replace(
        &pack_dir.join(LICENCE_FILE),
        listing.pack.licence.text.as_bytes(),
    )?;
    let manifest = manifest_of(listing);
    let text = serde_json::to_string_pretty(&manifest).expect("a pack manifest serializes");
    replace(
        &pack_dir.join(PACK_MANIFEST_FILE),
        format!("{text}\n").as_bytes(),
    )
}

fn manifest_of<'listing>(listing: &'listing PackListing<'_>) -> PackManifest<'listing> {
    PackManifest {
        source: listing.pack.source,
        commit: listing.pack.commit,
        tree: listing.pack.tree,
        licence: ManifestLicence {
            spdx: listing.pack.licence.spdx,
            file: LICENCE_FILE,
            sha256: sha256(listing.pack.licence.text),
        },
        skills: listing
            .skills
            .iter()
            .map(|listed| ManifestSkill {
                name: listed.skill.name,
                upstream_path: listed.skill.upstream_path,
                tree: listed.skill.tree,
                state: listed.state.as_str(),
                shadowed_by: match &listed.state {
                    SkillState::Shadowed { by } => Some(by.as_path()),
                    SkillState::Delivered | SkillState::Off => None,
                },
                files: listed
                    .skill
                    .files
                    .iter()
                    .map(|file| ManifestFile {
                        path: file.path,
                        sha256: sha256(file.text),
                    })
                    .collect(),
            })
            .collect(),
    }
}

/// The skills the manifest under `dir` says an earlier start delivered that `pack` no longer
/// holds, so a pin that drops one does not leave it listed. Only a plain directory name that
/// is not Torlek's own skill counts: the manifest is a file on disk, not an authority to
/// remove anything else. No manifest, or one that does not read, names none.
fn dropped_since_last_write(dir: &Path, pack: &Pack) -> Vec<String> {
    #[derive(Deserialize)]
    struct Earlier {
        skills: Vec<EarlierSkill>,
    }
    #[derive(Deserialize)]
    struct EarlierSkill {
        name: String,
    }
    let Ok(text) = fs::read_to_string(dir.join(PACK_DIR).join(PACK_MANIFEST_FILE)) else {
        return Vec::new();
    };
    let Ok(earlier) = serde_json::from_str::<Earlier>(&text) else {
        return Vec::new();
    };
    let bundle_dirs: BTreeSet<&str> = crate::documents()
        .iter()
        .filter_map(|document| document.path.split('/').next())
        .collect();
    earlier
        .skills
        .into_iter()
        .map(|skill| skill.name)
        .filter(|name| {
            let mut components = Path::new(name).components();
            matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
        })
        .filter(|name| name != PACK_DIR && !bundle_dirs.contains(name.as_str()))
        .filter(|name| pack.skills.iter().all(|skill| skill.name != name))
        .collect()
}

/// Writes every file of `skill` under `skill_dir`, then removes whatever else an earlier pin
/// left there. A link at `skill_dir` is removed first, never written through.
fn write_skill(root: &Path, skill_dir: &Path, skill: &PackSkill) -> Result<(), WriteError> {
    if fs::symlink_metadata(skill_dir).is_ok_and(|metadata| !metadata.is_dir()) {
        remove(skill_dir)?;
    }
    let mut expected = BTreeSet::new();
    for file in &skill.files {
        let path = skill_dir.join(file.path);
        let parent = path.parent().expect("a pack file path names a file");
        ensure_dir(root, parent)?;
        replace(&path, file.text.as_bytes())?;
        expected.insert(path);
    }
    remove_unlisted(skill_dir, &expected)
}

/// Removes every entry under `directory` that is not an `expected` file or a directory
/// holding one; a link is removed, never followed.
fn remove_unlisted(directory: &Path, expected: &BTreeSet<PathBuf>) -> Result<(), WriteError> {
    for entry in fs::read_dir(directory).map_err(io("read", directory))? {
        let path = entry.map_err(io("read", directory))?.path();
        let is_directory = fs::symlink_metadata(&path)
            .map_err(io("inspect", &path))?
            .is_dir();
        if is_directory && expected.iter().any(|file| file.starts_with(&path)) {
            remove_unlisted(&path, expected)?;
        } else if !expected.contains(&path) {
            remove(&path)?;
        }
    }
    Ok(())
}

/// Removes `path` whatever it is: a directory with its contents, a file, or a link itself.
fn remove(path: &Path) -> Result<(), WriteError> {
    let result = match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => Err(error),
    };
    result.map_err(io("remove", path))
}
