//! Where Torlek's masks helper is installed (#390, Theo 2026-11-13): a package puts the
//! helper and its units under `/usr/lib`, from the units this crate ships; on the install
//! script's path, which never calls sudo, `torlek setup engine-masks` puts the same helper and
//! units under `/usr/local/lib` and `/etc/systemd/system` (FHS, for files no package owns).
//! The two differ by the helper's path alone. Units in `/etc` shadow a package's, so the
//! setup step refuses where the package installs the helper, and readiness names both.

use std::io::ErrorKind;
use std::path::Path;

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
/// daemon runs. A package's needs no comparison: the package manager keeps it current.
pub fn masks_install(root: &Path, current_helper: &Path) -> Result<MasksInstall, String> {
    let at = |path: &str| root.join(path.trim_start_matches('/'));
    let is_packaged = exists(&at(&format!("{PACKAGED_UNIT_DIR}/{MASKS_SOCKET_UNIT}")))?;
    let mut has_set_up_units = false;
    let mut stale = Vec::new();
    for (name, text) in masks_units(SET_UP_HELPER) {
        let path = format!("{SET_UP_UNIT_DIR}/{name}");
        match read(&at(&path))? {
            Some(found) => {
                has_set_up_units = true;
                if found != text.as_bytes() {
                    stale.push(path);
                }
            }
            None => stale.push(path),
        }
    }
    // Only units in /etc shadow a package's; a helper an earlier setup left runs nowhere.
    if is_packaged {
        return Ok(if has_set_up_units {
            MasksInstall::Both
        } else {
            MasksInstall::Packaged
        });
    }
    let has_set_up_helper = match is_same_file(&at(SET_UP_HELPER), current_helper)? {
        Some(true) => true,
        Some(false) => {
            stale.push(SET_UP_HELPER.to_owned());
            true
        }
        None => {
            stale.push(SET_UP_HELPER.to_owned());
            false
        }
    };
    Ok(if has_set_up_units || has_set_up_helper {
        MasksInstall::SetUp { stale }
    } else {
        MasksInstall::Absent
    })
}

/// Whether `installed` has `current`'s contents, `None` when there is no `installed`. Sizes
/// first: a helper is megabytes, and the daemon asks at each readiness report.
fn is_same_file(installed: &Path, current: &Path) -> Result<Option<bool>, String> {
    let Some(installed_size) = size(installed)? else {
        return Ok(None);
    };
    let current_size = size(current)?
        .ok_or_else(|| format!("no {}: the helper to compare with", current.display()))?;
    if installed_size != current_size {
        return Ok(Some(false));
    }
    let contents = |path: &Path| {
        read(path)?.ok_or_else(|| format!("{} went away while it was read", path.display()))
    };
    Ok(Some(contents(installed)? == contents(current)?))
}

fn size(path: &Path) -> Result<Option<u64>, String> {
    match std::fs::metadata(path) {
        Ok(metadata) => Ok(Some(metadata.len())),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("cannot read {}: {error}", path.display())),
    }
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
    let units: Vec<String> = PACKAGED_UNITS
        .iter()
        .map(|(name, _)| format!("{SET_UP_UNIT_DIR}/{name}"))
        .collect();
    let helper_folders: Vec<String> = Path::new(SET_UP_HELPER)
        .ancestors()
        .skip(1)
        .take(2)
        .map(|folder| folder.display().to_string())
        .collect();
    let owner_drop_in = format!("{SET_UP_UNIT_DIR}/{MASKS_SOCKET_UNIT}.d");
    let set_up_files = units
        .iter()
        .chain([&SET_UP_HELPER.to_owned()])
        .map(|path| format!("[ ! -e {path} ]"))
        .collect::<Vec<_>>()
        .join(" && ");
    let packaged_socket = format!("{PACKAGED_UNIT_DIR}/{MASKS_SOCKET_UNIT}");
    let mut script = format!(
        "{PREAMBLE}\
         if {set_up_files}; then\n    \
         echo \"torlek setup engine-masks: the setup step installed nothing here: nothing to remove\" >&2\n    \
         exit 0\n\
         fi\n\
         if [ -e {SET_UP_UNIT_DIR}/{MASKS_SOCKET_UNIT} ]; then\n    \
         systemctl disable --now {MASKS_SOCKET_UNIT}\n\
         fi\n\
         # A connection's helper still running keeps the removed binary open.\n\
         systemctl stop '{instances}'\n",
        instances = MASKS_SERVICE_UNIT.replace("@.", "@*."),
    );
    for unit in &units {
        script.push_str(&format!("rm -f {unit}\n"));
    }
    script.push_str(&format!(
        "rm -f {SET_UP_HELPER}\n\
         # Only empty folders go: another program's files under them stay.\n\
         rmdir {folders} 2>/dev/null || true\n\
         # The owner the engines' setup gave the socket configures a package's socket too.\n\
         [ -e {packaged_socket} ] || rm -rf {owner_drop_in}\n\
         systemctl daemon-reload\n\
         if [ -e {packaged_socket} ]; then\n    \
         systemctl enable {MASKS_SOCKET_UNIT}\n    \
         systemctl restart {MASKS_SOCKET_UNIT}\n    \
         echo \"torlek setup engine-masks: the package's masks helper listens in its place\"\n\
         fi\n\
         echo \"torlek setup engine-masks: the setup step's masks helper is removed\"\n",
        folders = helper_folders.join(" "),
    ));
    script
}

const PREAMBLE: &str = "set -eu\n\
fail() {\n    echo \"torlek setup engine-masks: $*\" >&2\n    exit 1\n}\n\
[ \"$(id -u)\" = 0 ] || fail \"run it as root\"\n";
