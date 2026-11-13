//! `torlek-sandbox engine-masks` (#390): the root helper that shows a project's engine one
//! checkout at a time, through the session baseline's masks, locked where the engine runs.
//!
//! The namespaced tests stand in for the host with user namespaces only, as CI allows: the
//! "host" is a user namespace of the test's, the engine's unit a mount namespace it owns,
//! with an empty `/home` shared downstream, and the engine a user and mount namespace below
//! that, as rootlesskit makes one. Root of the "host" runs the helper; root of the engine's
//! user namespace, which a `--privileged` container is, probes what it sees. Needs
//! `unshare` and `nsenter` (util-linux) on `PATH`.
#![cfg(target_os = "linux")]

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::{MetadataExt, symlink};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

const LAUNCHER: &str = env!("CARGO_BIN_EXE_torlek-sandbox");

const ENGINE_USER: &str = "torlek-e3";

/// The project's checkouts on the "host": its main one and two parallel clones.
struct Checkouts {
    root: tempfile::TempDir,
}

impl Checkouts {
    fn new() -> Checkouts {
        let root = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
        let checkouts = Checkouts { root };
        for checkout in [
            checkouts.project_root(),
            checkouts.clone_named("one"),
            checkouts.clone_named("two"),
        ] {
            std::fs::create_dir_all(checkout.join(".git/hooks")).unwrap();
            std::fs::write(checkout.join(".git/hooks/pre-commit"), "#!/bin/sh\n").unwrap();
            std::fs::write(checkout.join(".git/config"), "[core]\n").unwrap();
            std::fs::write(checkout.join(".env"), "SECRET=1\n").unwrap();
            std::fs::write(checkout.join(".env.local"), "SECRET=2\n").unwrap();
            std::fs::create_dir_all(checkout.join("src")).unwrap();
            std::fs::write(checkout.join("src/main.rs"), "fn main() {}\n").unwrap();
        }
        for folder in ["probe", "stash"] {
            std::fs::create_dir_all(checkouts.root.path().join(folder)).unwrap();
        }
        checkouts
    }

    fn top(&self) -> &Path {
        self.root.path()
    }

    fn project_root(&self) -> PathBuf {
        self.top().join("home/owner/shop")
    }

    fn clones_dir(&self) -> PathBuf {
        self.top().join("home/owner/.parallel/shop")
    }

    fn clone_named(&self, name: &str) -> PathBuf {
        self.clones_dir().join(name)
    }

    /// The uid that owns the checkouts, as the process that made them sees it.
    fn owner_uid(&self) -> u32 {
        std::fs::metadata(self.top()).unwrap().uid()
    }

    /// A `systemctl` that names `main_pid` as the engine unit's main process.
    fn fake_systemctl(&self, main_pid: &str) -> PathBuf {
        let folder = self.top().join("bin");
        std::fs::create_dir_all(&folder).unwrap();
        let program = folder.join("systemctl");
        std::fs::write(
            &program,
            format!(
                "#!/bin/sh\n[ \"$*\" = 'show --property=MainPID --value \
                 torlek-engine@{ENGINE_USER}.service' ] || exit 3\necho {main_pid}\n"
            ),
        )
        .unwrap();
        make_executable(&program);
        folder
    }
}

fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// The helper's arguments, with the engine's file its setup writes: the engine runs as
/// `engine_uid` for the owner `owner_uid`.
fn helper_arguments(checkouts: &Checkouts, engine_uid: u32, owner_uid: u32) -> Vec<String> {
    let engines_dir = checkouts.top().join("engines");
    std::fs::create_dir_all(&engines_dir).unwrap();
    std::fs::write(
        engines_dir.join(format!("{ENGINE_USER}.conf")),
        format!(
            "# written by the engine's setup\nuid={engine_uid}\nowner_uid={owner_uid}\n\
             project_root={}\nclones_dir={}\n",
            checkouts.project_root().display(),
            checkouts.clones_dir().display()
        ),
    )
    .unwrap();
    vec!["engine-masks".to_owned(), engines_dir.display().to_string()]
}

/// Sends `request` to a helper run as `command`: its answer line and how it exited.
fn exchange(mut command: Command, request: &[u8]) -> (String, ExitStatus) {
    let (mut client, helper_end) = UnixStream::pair().unwrap();
    let mut helper = command
        .stdin(Stdio::from(std::os::fd::OwnedFd::from(helper_end)))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // The command keeps the helper's end open until dropped: no end of file reaches the
    // client otherwise when the helper exits without an answer.
    drop(command);
    client.write_all(request).unwrap();
    client.shutdown(std::net::Shutdown::Write).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(30)))
        .unwrap();
    let mut answer = String::new();
    let read = BufReader::new(&client).read_line(&mut answer);
    let status = helper.wait().unwrap();
    let mut errors = String::new();
    helper
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut errors)
        .unwrap();
    read.unwrap_or_else(|error| panic!("no answer ({error}), {status}: {errors}"));
    (answer, status)
}

fn ask(command: Command, request: &[u8]) -> String {
    exchange(command, request).0
}

/// The helper run directly, as the owner's own process: for what it refuses before it
/// touches a namespace.
fn direct_helper(checkouts: &Checkouts, owner_uid: u32) -> Command {
    let mut command = Command::new(LAUNCHER);
    command
        .args(helper_arguments(checkouts, 4242, owner_uid))
        .env("PATH", checkouts.fake_systemctl("0"));
    command
}

fn ask_directly(checkouts: &Checkouts, owner_uid: u32, request: &[u8]) -> String {
    ask(direct_helper(checkouts, owner_uid), request)
}

fn request_for(path: &Path) -> Vec<u8> {
    let mut request = format!("{ENGINE_USER} ").into_bytes();
    request.extend_from_slice(path.as_os_str().as_encoded_bytes());
    request.push(b'\n');
    request
}

#[test]
fn a_peer_other_than_the_owner_is_refused() {
    let checkouts = Checkouts::new();

    let answer = ask_directly(
        &checkouts,
        checkouts.owner_uid() + 1,
        &request_for(&checkouts.clone_named("one")),
    );

    assert!(answer.starts_with("refused: "), "{answer}");
    assert!(
        answer.contains("no engine of torlek-e3 is set up for uid"),
        "{answer}"
    );
}

#[test]
fn an_unreadable_engines_file_is_not_described_to_the_asker() {
    let checkouts = Checkouts::new();
    let mut command = direct_helper(&checkouts, checkouts.owner_uid());
    std::fs::write(checkouts.top().join("engines/torlek-e5.conf"), "uid=nine\n").unwrap();
    let request = format!("torlek-e5 {}\n", checkouts.clone_named("one").display());
    command.env("PATH", checkouts.fake_systemctl("0"));

    let answer = ask(command, request.as_bytes());

    assert!(
        answer.contains("no engine of torlek-e5 is set up"),
        "{answer}"
    );
    assert!(!answer.contains("engines/"), "{answer}");
    assert!(!answer.contains("nine"), "{answer}");
}

#[test]
fn a_path_that_is_not_one_of_the_projects_checkouts_is_refused() {
    let checkouts = Checkouts::new();
    let clones_dir = checkouts.clones_dir();
    for path in [
        PathBuf::from("/etc"),
        clones_dir.clone(),
        clones_dir.join("one/src"),
        clones_dir.join("one/../two"),
        PathBuf::from("one"),
        checkouts.top().join("home/owner"),
    ] {
        let answer = ask_directly(&checkouts, checkouts.owner_uid(), &request_for(&path));

        assert!(
            answer.starts_with("refused: "),
            "{}: {answer}",
            path.display()
        );
        assert!(
            answer.contains("is not the project's checkout nor one of its clones"),
            "{}: {answer}",
            path.display()
        );
    }
}

#[test]
fn a_clone_that_is_a_link_is_refused() {
    let checkouts = Checkouts::new();
    let link = checkouts.clone_named("link");
    symlink(checkouts.clone_named("one"), &link).unwrap();

    let answer = ask_directly(&checkouts, checkouts.owner_uid(), &request_for(&link));

    assert!(answer.starts_with("refused: "), "{answer}");
    assert!(answer.contains("is a symbolic link"), "{answer}");
}

#[test]
fn a_folder_without_git_is_refused() {
    let checkouts = Checkouts::new();
    let plain = checkouts.clone_named("plain");
    std::fs::create_dir_all(&plain).unwrap();

    let answer = ask_directly(&checkouts, checkouts.owner_uid(), &request_for(&plain));

    assert!(answer.starts_with("refused: "), "{answer}");
    assert!(answer.contains("holds no .git"), "{answer}");
}

#[test]
fn a_request_without_its_newline_is_refused() {
    let checkouts = Checkouts::new();
    let path = checkouts.clone_named("one");

    let mut request = request_for(&path);
    request.pop();

    let answer = ask_directly(&checkouts, checkouts.owner_uid(), &request);

    assert!(answer.starts_with("refused: "), "{answer}");
}

#[test]
fn a_request_longer_than_a_path_is_refused() {
    let checkouts = Checkouts::new();
    let mut request = format!("{ENGINE_USER} /").into_bytes();
    request.extend(std::iter::repeat_n(b'a', 5000));
    request.push(b'\n');

    let answer = ask_directly(&checkouts, checkouts.owner_uid(), &request);

    assert!(answer.starts_with("refused: "), "{answer}");
    assert!(answer.contains("is longer than"), "{answer}");
}

#[test]
fn an_engine_without_a_file_of_its_setup_is_refused() {
    let checkouts = Checkouts::new();
    let request = format!("torlek-e9 {}\n", checkouts.clone_named("one").display());

    let answer = ask_directly(&checkouts, checkouts.owner_uid(), request.as_bytes());

    assert!(answer.starts_with("refused: "), "{answer}");
    assert!(
        answer.contains("no engine of torlek-e9 is set up"),
        "{answer}"
    );
}

#[test]
fn an_engine_user_that_is_not_a_plain_name_is_refused() {
    let checkouts = Checkouts::new();
    let request = format!("../engines/x {}\n", checkouts.clone_named("one").display());

    let answer = ask_directly(&checkouts, checkouts.owner_uid(), request.as_bytes());

    assert!(answer.starts_with("refused: "), "{answer}");
    assert!(answer.contains("is not a plain name"), "{answer}");
}

/// The socket's unit instance must not fail for a request refused: only the asker learns.
#[test]
fn a_refusal_is_answered_without_the_helper_failing() {
    let checkouts = Checkouts::new();

    let (answer, status) = exchange(
        direct_helper(&checkouts, checkouts.owner_uid() + 1),
        &request_for(&checkouts.clone_named("one")),
    );

    assert!(answer.starts_with("refused: "), "{answer}");
    assert!(status.success(), "{status}");
}

#[test]
fn a_stopped_engine_is_refused() {
    let checkouts = Checkouts::new();

    let answer = ask_directly(
        &checkouts,
        checkouts.owner_uid(),
        &request_for(&checkouts.clone_named("one")),
    );

    assert!(answer.starts_with("refused: "), "{answer}");
    assert!(
        answer.contains("torlek-engine@torlek-e3.service is not running"),
        "{answer}"
    );
}

/// The "host", the engine's unit and the engine, as the module's documentation says.
struct World {
    checkouts: Checkouts,
    host: Child,
    unit_pid: String,
    engine_pid: String,
}

/// What the engine's unit shows of the host's `/home`.
#[derive(Clone, Copy)]
enum UnitView {
    /// An empty folder, as the engine's setup makes it.
    Hidden,
    /// The host's, as a unit without `ProtectHome` has it.
    Host,
    /// An empty folder with the host's clones folder bound in it, as engines set up before
    /// the masks have it.
    ClonesBound,
}

impl UnitView {
    fn word(self) -> &'static str {
        match self {
            UnitView::Hidden => "hidden",
            UnitView::Host => "host",
            UnitView::ClonesBound => "clones",
        }
    }
}

/// Run by `unshare -Urm` as the "host"'s root: the engine's unit makes its view of `/home`
/// (`$2`, a [`UnitView`]) and shares it downstream, and the engine below it takes that view
/// as rootlesskit does (`--propagation=rslave`).
const WORLD: &str = r#"
set -eu
top=$1
unshare -m sh -c '
    clones="$1/home/owner/.parallel/shop"
    case $2 in
    host) mount --bind "$1/home" "$1/home" ;;
    clones)
        mount --bind "$clones" "$1/stash"
        mount -t tmpfs -o mode=0755 tmpfs "$1/home"
        mkdir -p "$clones"
        mount --move "$1/stash" "$clones" ;;
    *) mount -t tmpfs -o mode=0755 tmpfs "$1/home" ;;
    esac
    mount --make-rshared "$1/home"
    unshare -Urm --propagation slave sh -c "echo \$\$ >\"\$1/engine.tmp\" && mv \"\$1/engine.tmp\" \"\$1/engine.pid\" && exec sleep 300" engine "$1" &
    echo $$ >"$1/unit.tmp" && mv "$1/unit.tmp" "$1/unit.pid"
    wait
' unit "$top" "$2" &
wait
"#;

impl World {
    fn new() -> World {
        World::with_unit_view(UnitView::Hidden)
    }

    fn with_unit_view(view: UnitView) -> World {
        let checkouts = Checkouts::new();
        let host = Command::new("unshare")
            .args(["-Urm", "sh", "-c", WORLD, "world"])
            .arg(checkouts.top())
            .arg(view.word())
            .stdin(Stdio::null())
            .spawn()
            .expect("unshare (util-linux) on PATH");
        let started = Instant::now();
        let pid_of = |name: &str| std::fs::read_to_string(checkouts.top().join(name));
        while pid_of("unit.pid").is_err() || pid_of("engine.pid").is_err() {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "the world did not start"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let unit_pid = pid_of("unit.pid").unwrap().trim().to_owned();
        let engine_pid = pid_of("engine.pid").unwrap().trim().to_owned();
        World {
            checkouts,
            host,
            unit_pid,
            engine_pid,
        }
    }

    /// Asks the helper, run as the "host"'s root, to mask `checkout`.
    fn mask(&self, checkout: &Path) -> String {
        self.mask_for(&self.unit_pid, checkout)
    }

    /// As [`World::mask`], with `main_pid` named as the unit's main process.
    fn mask_for(&self, main_pid: &str, checkout: &Path) -> String {
        let mut command = Command::new("nsenter");
        command
            .args([
                "-t",
                &self.host.id().to_string(),
                "-U",
                "-m",
                "--preserve-credentials",
                "--",
            ])
            .arg(LAUNCHER)
            .args(helper_arguments(&self.checkouts, 0, 0))
            .env("PATH", path_with(&self.checkouts.fake_systemctl(main_pid)));
        ask(command, &request_for(checkout))
    }

    /// Runs `script` as the "host"'s root.
    fn run_as_host_root(&self, script: &str) {
        let status = Command::new("nsenter")
            .args([
                "-t",
                &self.host.id().to_string(),
                "-U",
                "-m",
                "--preserve-credentials",
                "--",
            ])
            .args(["sh", "-ec", script, "root"])
            .arg(self.checkouts.top())
            .status()
            .unwrap();
        assert!(status.success(), "{script}: {status}");
    }

    /// What `script` prints run as root of the engine's user namespace, where a privileged
    /// container runs: one line per probe.
    fn probe(&self, script: &str) -> String {
        let output = Command::new("nsenter")
            .args([
                "-t",
                &self.engine_pid,
                "-U",
                "-m",
                "--preserve-credentials",
                "--",
            ])
            .args(["sh", "-c", script, "probe"])
            .arg(self.checkouts.top())
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).into_owned()
    }
}

impl Drop for World {
    fn drop(&mut self) {
        for pid in [&self.engine_pid, &self.unit_pid] {
            let _ = Command::new("kill").arg(pid).status();
        }
        let _ = self.host.kill();
        let _ = self.host.wait();
    }
}

/// `PATH` with `folder` first, so the helper finds the fake `systemctl` and the real tools.
fn path_with(folder: &Path) -> String {
    format!(
        "{}:{}",
        folder.display(),
        std::env::var("PATH").unwrap_or_default()
    )
}

#[test]
fn a_masked_clone_shows_the_engine_its_files_but_not_its_environment_files() {
    let world = World::new();

    let answer = world.mask(&world.checkouts.clone_named("one"));
    let seen = world.probe(
        r#"clone="$1/home/owner/.parallel/shop/one"
        echo "main=$(cat "$clone/src/main.rs")"
        echo "env=[$(cat "$clone/.env")]"
        echo "env.local=[$(cat "$clone/.env.local")]""#,
    );

    assert_eq!(answer, "ok\n");
    assert!(seen.contains("main=fn main() {}\n"), "{seen}");
    assert!(seen.contains("env=[]\n"), "{seen}");
    assert!(seen.contains("env.local=[]\n"), "{seen}");
}

#[test]
fn a_masked_clones_write_denials_are_read_only_to_the_engine() {
    let world = World::new();

    let answer = world.mask(&world.checkouts.clone_named("one"));
    let seen = world.probe(
        r#"clone="$1/home/owner/.parallel/shop/one"
        touch "$clone/.git/hooks/post-checkout" 2>/dev/null || echo hooks-read-only
        { echo x >>"$clone/.git/config"; } 2>/dev/null || echo config-read-only
        [ -d "$clone/.torlek" ] && echo torlek-folder
        touch "$clone/.torlek/checks" 2>/dev/null || echo torlek-read-only
        touch "$clone/src/new.rs" && echo source-writable"#,
    );

    assert_eq!(answer, "ok\n");
    for line in [
        "hooks-read-only",
        "config-read-only",
        "torlek-folder",
        "torlek-read-only",
        "source-writable",
    ] {
        assert!(seen.contains(&format!("{line}\n")), "{line}: {seen}");
    }
}

#[test]
fn root_of_the_engines_user_namespace_cannot_lift_the_masks() {
    let world = World::new();

    let answer = world.mask(&world.checkouts.clone_named("one"));
    let seen = world.probe(
        r#"clone="$1/home/owner/.parallel/shop/one"
        umount "$clone/.env" 2>/dev/null || echo env-stays
        umount -l "$clone/.git/hooks" 2>/dev/null || echo hooks-stay
        mount -o remount,bind,rw "$clone/.git/hooks" 2>/dev/null || echo hooks-stay-read-only
        mount --bind "$clone" "$1/probe" 2>/dev/null || echo bare-bind-refused
        mount --rbind "$clone" "$1/probe" && echo "rebound env=[$(cat "$1/probe/.env")]""#,
    );

    assert_eq!(answer, "ok\n");
    for line in [
        "env-stays",
        "hooks-stay",
        "hooks-stay-read-only",
        "bare-bind-refused",
        "rebound env=[]",
    ] {
        assert!(seen.contains(&format!("{line}\n")), "{line}: {seen}");
    }
}

#[test]
fn the_engine_sees_only_the_checkouts_it_was_asked_to_mask() {
    let world = World::new();

    let answer = world.mask(&world.checkouts.project_root());
    let seen = world.probe(
        r#"home="$1/home/owner"
        [ -e "$home/shop/src/main.rs" ] && echo main-checkout-seen
        echo "env=[$(cat "$home/shop/.env")]"
        [ -e "$home/.parallel/shop/one" ] || echo clone-unseen"#,
    );

    assert_eq!(answer, "ok\n");
    for line in ["main-checkout-seen", "env=[]", "clone-unseen"] {
        assert!(seen.contains(&format!("{line}\n")), "{line}: {seen}");
    }
}

#[test]
fn masking_again_hides_an_environment_file_made_since() {
    let world = World::new();
    let clone = world.checkouts.clone_named("one");
    assert_eq!(world.mask(&clone), "ok\n");
    std::fs::write(clone.join(".env.test"), "SECRET=3\n").unwrap();

    let answer = world.mask(&clone);
    let seen = world.probe(
        r#"clone="$1/home/owner/.parallel/shop/one"
        echo "env.test=[$(cat "$clone/.env.test")]"
        echo "mounts=$(grep -c " $clone " /proc/self/mountinfo)""#,
    );

    assert_eq!(answer, "ok\n");
    assert!(seen.contains("env.test=[]\n"), "{seen}");
    assert!(seen.contains("mounts=1\n"), "{seen}");
}

#[test]
fn a_clone_holding_a_mount_is_refused() {
    let world = World::new();
    world.run_as_host_root(r#"mount -t tmpfs tmpfs "$1/home/owner/.parallel/shop/one/src""#);

    let answer = world.mask(&world.checkouts.clone_named("one"));

    assert!(answer.starts_with("refused: "), "{answer}");
    assert!(answer.contains("holds a mount at"), "{answer}");
}

#[test]
fn a_unit_showing_the_hosts_checkouts_is_refused_without_touching_them() {
    for view in [UnitView::Host, UnitView::ClonesBound] {
        let world = World::with_unit_view(view);
        let clone = world.checkouts.clone_named("one");

        let answer = world.mask(&clone);

        assert!(answer.starts_with("refused: "), "{}: {answer}", view.word());
        assert!(
            answer.contains("run the engine's setup again"),
            "{}: {answer}",
            view.word()
        );
        assert!(!clone.join(".torlek").exists(), "{}", view.word());
    }
}

/// Mounts attached to a namespace a user namespace below owns are not locked there.
#[test]
fn a_unit_whose_main_process_has_a_user_namespace_of_its_own_is_refused() {
    let world = World::new();

    let answer = world.mask_for(&world.engine_pid, &world.checkouts.clone_named("one"));

    assert!(answer.starts_with("refused: "), "{answer}");
    assert!(
        answer.contains("runs in a user namespace of its own"),
        "{answer}"
    );
}
