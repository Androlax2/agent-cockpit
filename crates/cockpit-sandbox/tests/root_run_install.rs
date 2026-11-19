//! `torlek setup root-run` installs the root-run helper on the install script's path (#1387,
//! Theo 2026-11-19): its root step copies the owner's `torlek-sandbox` to a folder of root's
//! own, apart from the masks helper's copy, and refuses where a package installs the helper.
//! The steps run here as root of a user namespace. Needs `unshare` (util-linux).

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use cockpit_sandbox::{
    PACKAGED_ROOT_RUN_HELPER, SET_UP_ROOT_RUN_HELPER, root_run_removal_script,
    root_run_setup_script,
};

/// The masks helper's copy, which shares `/usr/local/lib/torlek`.
const MASKS_HELPER_COPY: &str = "/usr/local/lib/torlek/bin/torlek-sandbox";

/// `script` runs as root of a user namespace whose `/usr/local` is its own, `/usr/local/lib`
/// of mode `$LIB_MODE` starting as the folder the run before left, and whose `/usr/lib` holds a
/// package's helper when asked.
const STEP: &str = r#"set -eu
out=$1 script=$2 helper=$3 is_packaged=$4
mount -t tmpfs -o mode=755 none /usr/local
mkdir -m "$LIB_MODE" /usr/local/lib
[ ! -d "$out/root/usr/local/lib" ] || cp -a "$out/root/usr/local/lib/." /usr/local/lib/
if [ "$is_packaged" = yes ]; then
    mkdir -p "$out/upper" "$out/work"
    mount -t overlay overlay -o "lowerdir=/usr/lib,upperdir=$out/upper,workdir=$out/work" /usr/lib
    mkdir -p "$(dirname "$PACKAGED")"
    echo "the package's helper" >"$PACKAGED"
fi
status=0
sh -c "$script" torlek-setup "$helper" || status=$?
rm -rf "$out/root"
mkdir -p "$out/root/usr/local/lib"
cp -a /usr/local/lib/. "$out/root/usr/local/lib/"
exit "$status"
"#;

struct RootStep {
    dir: tempfile::TempDir,
    /// `/usr/local/lib`'s mode.
    lib_mode: &'static str,
}

struct Ran {
    is_success: bool,
    stdout: String,
    stderr: String,
    root: PathBuf,
}

impl RootStep {
    fn new() -> RootStep {
        let step = RootStep {
            dir: tempfile::tempdir().unwrap(),
            lib_mode: "755",
        };
        step.release("this release's helper");
        step
    }

    fn helper(&self) -> PathBuf {
        self.dir.path().join("torlek-sandbox")
    }

    /// The owner's `torlek-sandbox` becomes `text`, as `torlek update` would leave it.
    fn release(&self, text: &str) {
        std::fs::write(self.helper(), text).unwrap();
    }

    /// Leaves `path` under `/usr/local/lib` for the next run, as another step would.
    fn leave(&self, path: &str, text: &str) {
        let at = self
            .dir
            .path()
            .join("root")
            .join(path.trim_start_matches('/'));
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::fs::write(at, text).unwrap();
    }

    fn run(&self, script: &str, is_packaged: bool) -> Ran {
        let output = Command::new("unshare")
            .args(["-Urm", "sh", "-c", STEP, "step"])
            .arg(self.dir.path())
            .arg(script)
            .arg(self.helper())
            .arg(if is_packaged { "yes" } else { "no" })
            .env("PACKAGED", PACKAGED_ROOT_RUN_HELPER)
            .env("LIB_MODE", self.lib_mode)
            .output()
            .expect("unshare (util-linux) on PATH");
        Ran {
            is_success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            root: self.dir.path().join("root"),
        }
    }
}

impl Ran {
    fn at(&self, path: &str) -> PathBuf {
        self.root.join(path.trim_start_matches('/'))
    }

    fn file(&self, path: &str) -> Option<String> {
        std::fs::read_to_string(self.at(path)).ok()
    }

    fn mode(&self, path: &str) -> u32 {
        std::fs::metadata(self.at(path))
            .unwrap()
            .permissions()
            .mode()
            & 0o7777
    }
}

#[test]
fn the_setup_step_copies_the_owners_helper_where_root_alone_writes() {
    let step = RootStep::new();

    let ran = step.run(&root_run_setup_script(), false);

    assert!(ran.is_success, "{}", ran.stderr);
    assert_eq!(
        ran.file(SET_UP_ROOT_RUN_HELPER).as_deref(),
        Some("this release's helper")
    );
    for path in [
        SET_UP_ROOT_RUN_HELPER,
        "/usr/local/lib/torlek/root-run",
        "/usr/local/lib/torlek",
    ] {
        assert_eq!(ran.mode(path), 0o755, "{path}");
    }
    assert!(
        ran.stdout.contains(SET_UP_ROOT_RUN_HELPER),
        "{}",
        ran.stdout
    );
}

#[test]
fn running_the_setup_step_again_replaces_the_copy_with_this_release() {
    let step = RootStep::new();
    step.run(&root_run_setup_script(), false);
    step.release("the next release's helper");

    let ran = step.run(&root_run_setup_script(), false);

    assert!(ran.is_success, "{}", ran.stderr);
    assert_eq!(
        ran.file(SET_UP_ROOT_RUN_HELPER).as_deref(),
        Some("the next release's helper")
    );
}

#[test]
fn the_setup_step_where_the_package_installs_the_helper_changes_nothing() {
    let step = RootStep::new();

    let ran = step.run(&root_run_setup_script(), true);

    assert!(!ran.is_success);
    assert!(
        ran.stderr
            .contains("Torlek's package installs the root-run helper already"),
        "{}",
        ran.stderr
    );
    assert_eq!(ran.file(SET_UP_ROOT_RUN_HELPER), None);
}

#[test]
fn the_setup_step_below_a_folder_another_user_writes_installs_nothing() {
    for mode in ["2775", "1777"] {
        let step = RootStep {
            lib_mode: mode,
            ..RootStep::new()
        };

        let ran = step.run(&root_run_setup_script(), false);

        assert!(!ran.is_success, "{mode}");
        assert!(
            ran.stderr
                .contains("/usr/local/lib is writable by its group or other users"),
            "{mode}: {}",
            ran.stderr
        );
        assert_eq!(ran.file(SET_UP_ROOT_RUN_HELPER), None, "{mode}");
    }
}

#[test]
fn the_removal_step_removes_the_copy_and_keeps_the_masks_helpers() {
    let step = RootStep::new();
    step.leave(MASKS_HELPER_COPY, "the masks helper");
    step.run(&root_run_setup_script(), false);

    let ran = step.run(&root_run_removal_script(), false);

    assert!(ran.is_success, "{}", ran.stderr);
    assert_eq!(ran.file(SET_UP_ROOT_RUN_HELPER), None);
    assert!(!ran.at("/usr/local/lib/torlek/root-run").exists());
    assert_eq!(
        ran.file(MASKS_HELPER_COPY).as_deref(),
        Some("the masks helper")
    );
}

#[test]
fn the_removal_step_removes_its_empty_folders() {
    let step = RootStep::new();
    step.run(&root_run_setup_script(), false);

    let ran = step.run(&root_run_removal_script(), false);

    assert!(ran.is_success, "{}", ran.stderr);
    assert!(!ran.at("/usr/local/lib/torlek").exists());
}

#[test]
fn the_removal_step_where_nothing_was_set_up_changes_nothing() {
    let step = RootStep::new();

    let ran = step.run(&root_run_removal_script(), false);

    assert!(ran.is_success, "{}", ran.stderr);
    assert!(ran.stderr.contains("nothing to remove"), "{}", ran.stderr);
}

#[test]
fn each_step_refuses_to_run_but_as_root() {
    for script in [root_run_setup_script(), root_run_removal_script()] {
        let output = Command::new("sh")
            .args(["-c", &script, "torlek-setup"])
            .arg(Path::new("/nonexistent/torlek-sandbox"))
            .output()
            .unwrap();

        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("run it as root"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
