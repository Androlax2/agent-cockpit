//! Torlek's masks helper is installed by a package or by `torlek setup engine-masks` (#390):
//! the same helper and units on both paths, but for where they sit. Readiness reads which
//! one is installed and whether the setup step's copy still matches this release.

use std::path::{Path, PathBuf};
use std::process::Command;

use cockpit_containers::{
    ENGINES_DIR, MASKS_SOCKET, MasksInstall, PACKAGED_HELPER, PACKAGED_UNIT_DIR, SET_UP_HELPER,
    SET_UP_UNIT_DIR, masks_install, masks_removal_script, masks_setup_script, masks_units,
};

/// The packaged units, as the release ships them for the packages to install.
fn packaged_units() -> Vec<(&'static str, String)> {
    masks_units(PACKAGED_HELPER)
}

fn unit<'units>(units: &'units [(&'static str, String)], name: &str) -> &'units str {
    &units
        .iter()
        .find(|(unit_name, _)| *unit_name == name)
        .unwrap_or_else(|| panic!("no unit {name}"))
        .1
}

#[test]
fn the_helpers_socket_listens_where_the_daemon_asks_and_lets_only_root_connect() {
    let socket = unit(&packaged_units(), "torlek-engine-masks.socket").to_owned();

    assert!(
        socket.contains(&format!("\nListenStream={MASKS_SOCKET}\n")),
        "{socket}"
    );
    assert!(socket.contains("\nAccept=yes\n"), "{socket}");
    assert!(socket.contains("\nSocketMode=0600\n"), "{socket}");
    assert!(!socket.contains("SocketUser="), "{socket}");
}

#[test]
fn each_connection_runs_the_helper_on_the_engines_its_setups_record() {
    let service = unit(&packaged_units(), "torlek-engine-masks@.service").to_owned();

    assert!(
        service.contains(&format!(
            "\nExecStart={PACKAGED_HELPER} engine-masks {ENGINES_DIR}\n"
        )),
        "{service}"
    );
    assert!(service.contains("\nStandardInput=socket\n"), "{service}");
}

#[test]
fn the_setup_steps_units_differ_from_the_packages_by_the_helpers_path_alone() {
    let set_up = masks_units(SET_UP_HELPER);

    let normalised: Vec<(&str, String)> = set_up
        .iter()
        .map(|(name, text)| (*name, text.replace(SET_UP_HELPER, PACKAGED_HELPER)))
        .collect();

    assert_eq!(normalised, packaged_units());
    assert!(unit(&set_up, "torlek-engine-masks@.service").contains(SET_UP_HELPER));
}

/// A host root under a temporary folder, with the files each install path writes.
struct Host {
    root: tempfile::TempDir,
    current_helper: PathBuf,
}

impl Host {
    fn new() -> Host {
        let root = tempfile::tempdir().unwrap();
        let current_helper = root
            .path()
            .join("home/owner/.local/lib/torlek/bin/torlek-sandbox");
        write(&current_helper, "this release's helper");
        Host {
            root,
            current_helper,
        }
    }

    fn at(&self, path: &str) -> PathBuf {
        self.root.path().join(path.trim_start_matches('/'))
    }

    fn install_package(&self) {
        for (name, text) in masks_units(PACKAGED_HELPER) {
            write(&self.at(&format!("{PACKAGED_UNIT_DIR}/{name}")), &text);
        }
        write(&self.at(PACKAGED_HELPER), "the package's helper");
    }

    fn run_setup_step(&self) {
        for (name, text) in masks_units(SET_UP_HELPER) {
            write(&self.at(&format!("{SET_UP_UNIT_DIR}/{name}")), &text);
        }
        write(&self.at(SET_UP_HELPER), "this release's helper");
    }

    fn install(&self) -> MasksInstall {
        masks_install(self.root.path(), &self.current_helper).unwrap()
    }
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

#[test]
fn a_host_without_either_install_has_no_helper() {
    let host = Host::new();

    assert_eq!(host.install(), MasksInstall::Absent);
}

#[test]
fn a_packages_helper_is_installed() {
    let host = Host::new();
    host.install_package();

    assert_eq!(host.install(), MasksInstall::Packaged);
}

#[test]
fn the_setup_steps_helper_that_matches_this_release_is_installed() {
    let host = Host::new();
    host.run_setup_step();

    assert_eq!(host.install(), MasksInstall::SetUp { stale: Vec::new() });
}

#[test]
fn the_setup_steps_copy_of_an_earlier_helper_is_stale() {
    let host = Host::new();
    host.run_setup_step();
    write(&host.current_helper, "a newer release's helper");

    let MasksInstall::SetUp { stale } = host.install() else {
        panic!("{:?}", host.install());
    };

    assert_eq!(stale, [SET_UP_HELPER.to_owned()]);
}

#[test]
fn a_setup_unit_that_differs_from_this_releases_or_is_missing_is_stale() {
    let host = Host::new();
    host.run_setup_step();
    let service = format!("{SET_UP_UNIT_DIR}/torlek-engine-masks@.service");
    write(&host.at(&service), "[Service]\nExecStart=/elsewhere\n");
    let socket = format!("{SET_UP_UNIT_DIR}/torlek-engine-masks.socket");
    std::fs::remove_file(host.at(&socket)).unwrap();
    write(&host.at(&format!("{SET_UP_UNIT_DIR}/unrelated.socket")), "");

    let MasksInstall::SetUp { stale } = host.install() else {
        panic!("{:?}", host.install());
    };

    assert_eq!(stale, [socket, service]);
}

#[test]
fn a_packages_helper_is_installed_whatever_the_daemon_runs() {
    let host = Host::new();
    host.install_package();
    std::fs::remove_file(&host.current_helper).unwrap();

    assert_eq!(host.install(), MasksInstall::Packaged);
}

#[test]
fn a_setup_steps_helper_left_beside_a_package_without_its_units_shadows_nothing() {
    let host = Host::new();
    host.install_package();
    write(&host.at(SET_UP_HELPER), "an earlier release's helper");

    assert_eq!(host.install(), MasksInstall::Packaged);
}

#[test]
fn the_setup_steps_helper_of_another_size_is_stale() {
    let host = Host::new();
    host.run_setup_step();
    write(
        &host.at(SET_UP_HELPER),
        "a longer helper of an earlier release",
    );

    let MasksInstall::SetUp { stale } = host.install() else {
        panic!("{:?}", host.install());
    };

    assert_eq!(stale, [SET_UP_HELPER.to_owned()]);
}

#[test]
fn a_setup_steps_copy_beside_the_packages_shadows_it() {
    let host = Host::new();
    host.install_package();
    host.run_setup_step();

    assert_eq!(host.install(), MasksInstall::Both);
}

/// A host's root for the setup step: `script` runs as root of a user namespace whose
/// `/etc/systemd/system`, `/usr/lib/systemd/system` and `/usr/local/lib` are empty, with a
/// fake `systemctl` first on `PATH` that logs its arguments. Needs `unshare` (util-linux).
struct RootStep {
    dir: tempfile::TempDir,
}

/// What a run of a root step left.
struct Ran {
    is_success: bool,
    stderr: String,
    /// The host's root as the step left it: those three folders, copied out.
    root: PathBuf,
    systemctl: String,
}

const FOLDERS: [&str; 3] = [
    "/etc/systemd/system",
    "/usr/lib/systemd/system",
    "/usr/local/lib",
];

const STEP: &str = r#"set -eu
out=$1 script=$2 helper=$3 is_packaged=$4
for folder in /etc/systemd/system /usr/lib/systemd/system /usr/local/lib; do
    mount -t tmpfs none "$folder"
    [ ! -d "$out/root$folder" ] || cp -a "$out/root$folder/." "$folder/"
done
if [ "$is_packaged" = yes ]; then
    echo '[Socket]' >/usr/lib/systemd/system/torlek-engine-masks.socket
fi
status=0
PATH="$out/fakes:$PATH" sh -c "$script" torlek-setup "$helper" || status=$?
rm -rf "$out/root"
for folder in /etc/systemd/system /usr/lib/systemd/system /usr/local/lib; do
    mkdir -p "$out/root$folder"
    cp -a "$folder/." "$out/root$folder/"
done
exit "$status"
"#;

impl RootStep {
    fn new() -> RootStep {
        let dir = tempfile::tempdir().unwrap();
        let systemctl = dir.path().join("fakes/systemctl");
        write(
            &systemctl,
            &format!(
                "#!/bin/sh\necho \"$*\" >>'{}'\n",
                dir.path().join("systemctl.log").display()
            ),
        );
        std::fs::set_permissions(
            &systemctl,
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();
        write(&dir.path().join("torlek-sandbox"), "this release's helper");
        RootStep { dir }
    }

    fn helper(&self) -> PathBuf {
        self.dir.path().join("torlek-sandbox")
    }

    /// Runs `script` on the host, a package's socket unit there when `is_packaged`; each run
    /// starts from the folders the one before left.
    fn run(&self, script: &str, is_packaged: bool) -> Ran {
        let output = Command::new("unshare")
            .args(["-Urm", "sh", "-c", STEP, "step"])
            .arg(self.dir.path())
            .arg(script)
            .arg(self.helper())
            .arg(if is_packaged { "yes" } else { "no" })
            .output()
            .expect("unshare (util-linux) on PATH");
        Ran {
            is_success: output.status.success(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            root: self.dir.path().join("root"),
            systemctl: std::fs::read_to_string(self.dir.path().join("systemctl.log"))
                .unwrap_or_default(),
        }
    }
}

impl Ran {
    fn file(&self, path: &str) -> Option<String> {
        std::fs::read_to_string(self.root.join(path.trim_start_matches('/'))).ok()
    }
}

#[test]
fn the_setup_step_installs_the_helper_and_its_units_then_listens() {
    let step = RootStep::new();

    let ran = step.run(&masks_setup_script(), false);

    assert!(ran.is_success, "{}", ran.stderr);
    for (name, text) in masks_units(SET_UP_HELPER) {
        assert_eq!(
            ran.file(&format!("{SET_UP_UNIT_DIR}/{name}")),
            Some(text),
            "{name}"
        );
    }
    assert_eq!(
        ran.file(SET_UP_HELPER).as_deref(),
        Some("this release's helper")
    );
    assert_eq!(
        ran.systemctl,
        "daemon-reload\nenable torlek-engine-masks.socket\nrestart torlek-engine-masks.socket\n"
    );
    assert_eq!(
        masks_install(&ran.root, &step.helper()).unwrap(),
        MasksInstall::SetUp { stale: Vec::new() }
    );
}

#[test]
fn the_setup_steps_helper_is_roots_and_runs_for_no_one_else_to_change() {
    let step = RootStep::new();

    let ran = step.run(&masks_setup_script(), false);

    let mode = std::os::unix::fs::PermissionsExt::mode(
        &std::fs::metadata(ran.root.join(SET_UP_HELPER.trim_start_matches('/')))
            .unwrap()
            .permissions(),
    );
    assert_eq!(mode & 0o7777, 0o755);
}

#[test]
fn the_setup_step_where_the_package_installs_the_helper_changes_nothing() {
    let step = RootStep::new();

    let ran = step.run(&masks_setup_script(), true);

    assert!(!ran.is_success);
    assert!(
        ran.stderr
            .contains("Torlek's package installs the masks helper already"),
        "{}",
        ran.stderr
    );
    assert_eq!(ran.file(SET_UP_HELPER), None);
    assert_eq!(ran.systemctl, "");
}

#[test]
fn removing_the_setup_steps_helper_leaves_none_of_its_files() {
    let step = RootStep::new();
    step.run(&masks_setup_script(), false);
    std::fs::remove_file(step.dir.path().join("systemctl.log")).unwrap();

    let ran = step.run(&masks_removal_script(), false);

    assert!(ran.is_success, "{}", ran.stderr);
    for folder in FOLDERS {
        let left: Vec<_> = walk(&ran.root.join(folder.trim_start_matches('/')));
        assert!(left.is_empty(), "{folder}: {left:?}");
    }
    assert_eq!(
        ran.systemctl,
        "disable --now torlek-engine-masks.socket\nstop torlek-engine-masks@*.service\n\
         daemon-reload\n"
    );
}

#[test]
fn removing_the_setup_steps_helper_beside_a_package_restarts_the_packages() {
    let step = RootStep::new();
    step.run(&masks_setup_script(), false);
    std::fs::remove_file(step.dir.path().join("systemctl.log")).unwrap();

    let ran = step.run(&masks_removal_script(), true);

    assert!(ran.is_success, "{}", ran.stderr);
    assert_eq!(
        ran.systemctl,
        "disable --now torlek-engine-masks.socket\nstop torlek-engine-masks@*.service\n\
         daemon-reload\nenable torlek-engine-masks.socket\nrestart torlek-engine-masks.socket\n"
    );
    assert!(
        ran.file(&format!("{PACKAGED_UNIT_DIR}/torlek-engine-masks.socket"))
            .is_some()
    );
}

#[test]
fn removing_where_the_setup_step_installed_nothing_changes_nothing() {
    let step = RootStep::new();

    let ran = step.run(&masks_removal_script(), true);

    assert!(ran.is_success, "{}", ran.stderr);
    assert!(ran.stderr.contains("nothing to remove"), "{}", ran.stderr);
    assert_eq!(ran.systemctl, "");
}

#[test]
fn removing_the_last_helper_removes_the_owner_its_engines_gave_its_socket() {
    let step = RootStep::new();
    step.run(&masks_setup_script(), false);
    write(
        &step.dir.path().join(format!(
            "root{SET_UP_UNIT_DIR}/torlek-engine-masks.socket.d/owner.conf"
        )),
        "[Socket]\nSocketUser=theo\n",
    );

    let ran = step.run(&masks_removal_script(), false);

    assert!(ran.is_success, "{}", ran.stderr);
    let left = walk(&ran.root.join(SET_UP_UNIT_DIR.trim_start_matches('/')));
    assert!(left.is_empty(), "{left:?}");
}

#[test]
fn removing_the_setup_steps_helper_beside_a_package_keeps_the_owner_its_engines_gave_its_socket() {
    let step = RootStep::new();
    step.run(&masks_setup_script(), false);
    let owner = format!("{SET_UP_UNIT_DIR}/torlek-engine-masks.socket.d/owner.conf");
    write(
        &step.dir.path().join(format!("root{owner}")),
        "[Socket]\nSocketUser=theo\n",
    );

    let ran = step.run(&masks_removal_script(), true);

    assert!(ran.is_success, "{}", ran.stderr);
    assert!(ran.file(&owner).is_some());
}

/// Every file under `folder`.
fn walk(folder: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    entries
        .flat_map(|entry| {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path)
            } else {
                vec![path]
            }
        })
        .collect()
}

/// Theo, 2026-11-13: the packages and `torlek setup engine-masks` install the same files, the
/// helper's path alone told apart. The packages are built here as the release builds them:
/// a `tools/release.py` bundle whose `torlek-sandbox` is the setup step's helper, the .deb
/// from `packaging/deb/build.py` and the AUR recipe's own `package()`
/// (tests/package_roots.py). Needs python3 and bash.
#[test]
fn the_packages_install_the_files_the_setup_step_installs_under_their_own_paths() {
    let step = RootStep::new();
    let ran = step.run(&masks_setup_script(), false);
    assert!(ran.is_success, "{}", ran.stderr);
    let set_up = files_under(&ran.root, &[SET_UP_UNIT_DIR, "/usr/local/lib"]);
    let as_packaged: Files = set_up
        .into_iter()
        .map(|(path, contents)| (packaged_path(&path), packaged_contents(contents)))
        .collect();
    assert!(as_packaged.len() >= 3, "{:?}", as_packaged.keys());

    for (package, root) in package_roots(&step) {
        let packaged = files_under(&root, &[PACKAGED_UNIT_DIR, PACKAGED_HELPER]);

        assert_eq!(packaged, as_packaged, "{package}");
    }
}

type Files = std::collections::BTreeMap<String, Vec<u8>>;

/// Every file under each of `paths` (a file or a folder) of the host at `root`, by its path
/// on that host.
fn files_under(root: &Path, paths: &[&str]) -> Files {
    let mut files = Files::new();
    for path in paths {
        let at = root.join(path.trim_start_matches('/'));
        let found = if at.is_file() { vec![at] } else { walk(&at) };
        for file in found {
            let on_host = format!("/{}", file.strip_prefix(root).unwrap().display());
            files.insert(on_host, std::fs::read(&file).unwrap());
        }
    }
    files
}

fn packaged_path(path: &str) -> String {
    if path == SET_UP_HELPER {
        return PACKAGED_HELPER.to_owned();
    }
    let unit = path
        .strip_prefix(&format!("{SET_UP_UNIT_DIR}/"))
        .unwrap_or_else(|| panic!("the setup step installed {path}, outside its own paths"));
    format!("{PACKAGED_UNIT_DIR}/{unit}")
}

fn packaged_contents(contents: Vec<u8>) -> Vec<u8> {
    match String::from_utf8(contents) {
        Ok(text) => text.replace(SET_UP_HELPER, PACKAGED_HELPER).into_bytes(),
        Err(binary) => binary.into_bytes(),
    }
}

/// Each package's installed tree, built from a bundle whose `torlek-sandbox` is `step`'s helper.
fn package_roots(step: &RootStep) -> Vec<(String, PathBuf)> {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let work = step.dir.path().join("packages");
    std::fs::create_dir(&work).unwrap();
    let output = Command::new("python3")
        .arg("-I")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/package_roots.py"))
        .arg(&repository)
        .arg(&work)
        .arg(step.helper())
        .output()
        .expect("python3 on PATH");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let roots: Vec<(String, PathBuf)> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| {
            let (name, root) = line.split_once(' ').unwrap();
            (name.to_owned(), PathBuf::from(root))
        })
        .collect();
    assert_eq!(
        roots
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        ["deb", "aur"]
    );
    roots
}
