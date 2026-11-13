//! Where Torlek's masks helper is installed (#390, Theo 2026-11-13): a package puts the
//! helper and its units under `/usr/lib`, from the units this crate ships; on the install
//! script's path, which never calls sudo, `torlek setup engine-masks` puts the same helper and
//! units under `/usr/local/lib` and `/etc/systemd/system` (FHS, for files no package owns).
//! The two differ by the helper's path alone. Units in `/etc` shadow a package's, so the
//! setup step refuses where the package installs the helper, and readiness names both.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::MASKS_SOCKET_UNIT;

/// Each connection's unit.
pub const MASKS_SERVICE_UNIT: &str = "torlek-engine-masks@.service";

/// The helper a package installs: the release bundle's own, under `/usr/lib/torlek`.
pub const PACKAGED_HELPER: &str = "/usr/lib/torlek/bin/torlek-sandbox";

/// Where a package installs the helper's units.
pub const PACKAGED_UNIT_DIR: &str = "/usr/lib/systemd/system";

/// The setup step's root-owned copy of the owner's `torlek-sandbox`: the owner can write
/// their own install, and root must not run what the owner can change.
pub const SET_UP_HELPER: &str = "/usr/local/lib/torlek/bin/torlek-sandbox";

/// Where the setup step writes the helper's units.
pub const SET_UP_UNIT_DIR: &str = "/etc/systemd/system";

/// The units as the packages install them, running `PACKAGED_HELPER`.
const PACKAGED_UNITS: [(&str, &str); 2] = [
    (
        MASKS_SOCKET_UNIT,
        include_str!("../units/torlek-engine-masks.socket"),
    ),
    (
        MASKS_SERVICE_UNIT,
        include_str!("../units/torlek-engine-masks@.service"),
    ),
];

/// The helper's units, each by name, running `helper`.
pub fn masks_units(helper: &str) -> Vec<(&'static str, String)> {
    PACKAGED_UNITS
        .iter()
        .map(|(name, text)| (*name, text.replace(PACKAGED_HELPER, helper)))
        .collect()
}

/// Which install of the helper a host has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MasksInstall {
    Absent,
    Packaged,
    /// The setup step's, with each of its files that is missing or differs from this
    /// release's: running the step again replaces them.
    SetUp {
        stale: Vec<String>,
    },
    /// The setup step's beside a package's, whose units it shadows.
    Both,
}

/// Which install of the helper the host under `root` (`/` but in tests) has, the setup
/// step's compared with this release's units and `current_helper`, the `torlek-sandbox` the
/// daemon runs.
pub fn masks_install(root: &Path, current_helper: &Path) -> Result<MasksInstall, String> {
    let at = |path: &str| root.join(path.trim_start_matches('/'));
    let is_packaged = exists(&at(&format!("{PACKAGED_UNIT_DIR}/{MASKS_SOCKET_UNIT}")))?;
    let mut expected: Vec<(String, Vec<u8>)> = masks_units(SET_UP_HELPER)
        .into_iter()
        .map(|(name, text)| (format!("{SET_UP_UNIT_DIR}/{name}"), text.into_bytes()))
        .collect();
    let current = read(current_helper)?.ok_or_else(|| {
        format!(
            "no {}: the helper to compare with",
            current_helper.display()
        )
    })?;
    expected.push((SET_UP_HELPER.to_owned(), current));
    let mut is_set_up = false;
    let mut stale = Vec::new();
    for (path, contents) in expected {
        match read(&at(&path))? {
            Some(found) => {
                is_set_up = true;
                if found != contents {
                    stale.push(path);
                }
            }
            None => stale.push(path),
        }
    }
    Ok(match (is_packaged, is_set_up) {
        (false, false) => MasksInstall::Absent,
        (true, false) => MasksInstall::Packaged,
        (false, true) => MasksInstall::SetUp { stale },
        (true, true) => MasksInstall::Both,
    })
}

fn exists(path: &Path) -> Result<bool, String> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("cannot read {}: {error}", path.display())),
    }
}

fn read(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match std::fs::read(path) {
        Ok(contents) => Ok(Some(contents)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("cannot read {}: {error}", path.display())),
    }
}

/// The root step `torlek setup engine-masks` runs once with sudo, the helper to copy as its
/// first argument. Running it again replaces the helper and units with this release's.
pub fn masks_setup_script() -> String {
    let mut script = format!(
        "{PREAMBLE}\
         helper=$1\n\
         [ ! -e {PACKAGED_UNIT_DIR}/{MASKS_SOCKET_UNIT} ] ||\n    \
         fail \"Torlek's package installs the masks helper already: nothing to set up\"\n\
         [ -f \"$helper\" ] || fail \"no helper at $helper\"\n\
         install -D -o root -g root -m 0755 \"$helper\" {SET_UP_HELPER}\n"
    );
    for (name, text) in masks_units(SET_UP_HELPER) {
        script.push_str(&format!(
            "cat >{SET_UP_UNIT_DIR}/{name} <<'UNIT'\n{text}UNIT\n"
        ));
    }
    script.push_str(&format!(
        "systemctl daemon-reload\n\
         systemctl enable {MASKS_SOCKET_UNIT}\n\
         # A socket that ran before keeps listening as its earlier units said.\n\
         systemctl restart {MASKS_SOCKET_UNIT}\n\
         echo \"torlek setup engine-masks: Torlek's masks helper listens on {socket}\"\n",
        socket = crate::MASKS_SOCKET,
    ));
    script
}

/// The root step `torlek setup engine-masks --uninstall` runs: it removes the setup step's
/// files alone, and a package's helper, shadowed until then, listens in their place.
pub fn masks_removal_script() -> String {
    let units: Vec<PathBuf> = PACKAGED_UNITS
        .iter()
        .map(|(name, _)| Path::new(SET_UP_UNIT_DIR).join(name))
        .collect();
    let mut script = format!(
        "{PREAMBLE}\
         if [ -e {SET_UP_UNIT_DIR}/{MASKS_SOCKET_UNIT} ]; then\n    \
         systemctl disable --now {MASKS_SOCKET_UNIT}\n\
         fi\n"
    );
    for unit in &units {
        script.push_str(&format!("rm -f {}\n", unit.display()));
    }
    script.push_str(&format!(
        "rm -f {SET_UP_HELPER}\n\
         # Only empty folders go: another program's files under them stay.\n\
         rmdir /usr/local/lib/torlek/bin /usr/local/lib/torlek 2>/dev/null || true\n\
         systemctl daemon-reload\n\
         if [ -e {PACKAGED_UNIT_DIR}/{MASKS_SOCKET_UNIT} ]; then\n    \
         systemctl enable {MASKS_SOCKET_UNIT}\n    \
         systemctl restart {MASKS_SOCKET_UNIT}\n    \
         echo \"torlek setup engine-masks: the package's masks helper listens in its place\"\n\
         fi\n\
         echo \"torlek setup engine-masks: the setup step's masks helper is removed\"\n"
    ));
    script
}

const PREAMBLE: &str = "set -eu\n\
fail() {\n    echo \"torlek setup engine-masks: $*\" >&2\n    exit 1\n}\n\
[ \"$(id -u)\" = 0 ] || fail \"run it as root\"\n";
