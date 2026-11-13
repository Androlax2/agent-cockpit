//! `torlek-sandbox engine-masks ENGINES_DIR` (#390): the root helper that shows a project's
//! engine one checkout, through the masks a session's sandbox puts on it, locked where the
//! engine runs. Theo chose it on 2026-11-13 (option C): a clone made after the engine started
//! is masked without a root step.
//!
//! A root-owned copy runs from a socket-activated unit, once per connection, the connection
//! as its standard input and output. The request is an engine's user, a space, one checkout's
//! path and a newline; the answer `ok` and a newline once that engine sees the checkout
//! masked, or `refused: `, the reason and a newline. Each engine's setup, a root step, writes
//! `ENGINES_DIR/<user>.conf` (see [`Engine::parse`]); the helper answers only the uid that
//! file names as the engine's owner (`SO_PEERCRED`).
//!
//! Only the engine's project root or a folder directly in its clones folder is masked: a
//! folder, reached without a symbolic link, the owner's, holding a `.git` and no mount. The
//! engine's unit must show it no checkout of its own; one that shows the host's clones
//! folder, as the setup still does, is refused. The helper builds the masked checkout in a mount
//! namespace of its own, clones that tree, and moves it into the unit's mount namespace at
//! the checkout's path. The unit's view is shared downstream to the engine's user namespace,
//! so the tree arrives there as one locked unit: root of that namespace, which a privileged
//! container is, can neither unmount a mask, nor make one writable, nor bind the checkout
//! without its masks. Asking again replaces the tree, masking an environment file made since.

use std::ffi::{OsStr, OsString};
use std::io::{ErrorKind, Read, Write};
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixStream;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use cockpit_sandbox::CheckoutMasks;
use rustix::fs::{AtFlags, CWD, FileType, Mode, OFlags, StatVfsMountFlags};
use rustix::mount::{
    MountFlags, MountPropagationFlags, MoveMountFlags, OpenTreeFlags, UnmountFlags,
};
use rustix::thread::{LinkNameSpaceType, UnshareFlags};

use crate::plan::{self, Mask};

/// The longest request before its newline: an engine's user, a space and a path the kernel
/// takes.
const MAX_REQUEST: usize = 33 + 4096;

/// How long the owner's side has to send its request.
const DEADLINE: Duration = Duration::from_secs(10);

/// One engine, as its setup recorded it for the helper.
#[derive(Debug)]
pub struct Engine {
    user: String,
    uid: u32,
    owner_uid: u32,
    project_root: PathBuf,
    clones_dir: PathBuf,
}

/// The folder of the engines' files, the helper's one argument.
pub fn engines_dir(arguments: &[OsString]) -> Result<PathBuf, String> {
    let [folder] = arguments else {
        return Err("usage: torlek-sandbox engine-masks ENGINES_DIR".to_owned());
    };
    plain_path("the engines' folder", folder)
}

impl Engine {
    /// The engine of `user`, from its file in `engines_dir`.
    fn load(engines_dir: &Path, user: &str) -> Result<Engine, String> {
        let path = engines_dir.join(format!("{user}.conf"));
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                return Err(format!("no engine of {user} is set up"));
            }
            Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
        };
        Engine::parse(user, &text).map_err(|reason| format!("{}: {reason}", path.display()))
    }

    /// An engine's file: `uid=`, `owner_uid=`, `project_root=` and `clones_dir=` lines, each
    /// once; blank lines and `#` comments aside, nothing else.
    pub fn parse(user: &str, text: &str) -> Result<Engine, String> {
        let mut uid = None;
        let mut owner_uid = None;
        let mut project_root = None;
        let mut clones_dir = None;
        for line in text.lines() {
            if line.trim().is_empty() || line.starts_with('#') {
                continue;
            }
            let (key, value) = line
                .split_once('=')
                .ok_or_else(|| format!("the line {line:?} is not KEY=VALUE"))?;
            let slot = match key {
                "uid" => &mut uid,
                "owner_uid" => &mut owner_uid,
                "project_root" => &mut project_root,
                "clones_dir" => &mut clones_dir,
                _ => return Err(format!("unknown key {key:?}")),
            };
            if slot.replace(value).is_some() {
                return Err(format!("{key} is set twice"));
            }
        }
        let uid = parse_uid("uid", uid)?;
        let owner_uid = parse_uid("owner_uid", owner_uid)?;
        let project_root = parse_folder("project_root", project_root)?;
        let clones_dir = parse_folder("clones_dir", clones_dir)?;
        // Showing the engine one would take the trees shown inside it away.
        if project_root.starts_with(&clones_dir) || clones_dir.starts_with(&project_root) {
            return Err(format!(
                "the clones folder {} and the project's checkout {} hold one another",
                clones_dir.display(),
                project_root.display()
            ));
        }
        Ok(Engine {
            user: user.to_owned(),
            uid,
            owner_uid,
            project_root,
            clones_dir,
        })
    }

    fn unit_name(&self) -> String {
        format!("torlek-engine@{}.service", self.user)
    }

    /// `request` as one of the project's checkouts and its folder, or why it is not one.
    fn checkout(&self, request: &[u8]) -> Result<(PathBuf, OwnedFd), String> {
        let path = PathBuf::from(OsStr::from_bytes(request));
        let is_checkout = is_plain(&path)
            && (path == self.project_root || path.parent() == Some(self.clones_dir.as_path()));
        if !is_checkout {
            return Err(format!(
                "{} is not the project's checkout nor one of its clones",
                path.display()
            ));
        }
        let folder = open_folder(&path)?;
        let owner = rustix::fs::fstat(&folder)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?
            .st_uid;
        if owner != self.owner_uid {
            return Err(format!(
                "{} belongs to uid {owner}, not the owner's uid {}",
                path.display(),
                self.owner_uid
            ));
        }
        match rustix::fs::statat(&folder, ".git", AtFlags::SYMLINK_NOFOLLOW) {
            Ok(_) => Ok((path, folder)),
            Err(rustix::io::Errno::NOENT) => Err(format!("{} holds no .git", path.display())),
            Err(error) => Err(format!("cannot read {}/.git: {error}", path.display())),
        }
    }

    /// The engine's unit, through its main process: the engine's user's, in a mount
    /// namespace of its own but in the helper's user namespace. Mounts the helper attaches
    /// to a mount namespace a user namespace below owns would arrive unlocked there.
    fn unit(&self) -> Result<Unit, String> {
        let unit = self.unit_name();
        let output = std::process::Command::new("systemctl")
            .args(["show", "--property=MainPID", "--value", &unit])
            .output()
            .map_err(|error| format!("cannot run systemctl: {error}"))?;
        let text = String::from_utf8_lossy(&output.stdout);
        let pid: u32 = text
            .trim()
            .parse()
            .ok()
            .filter(|_| output.status.success())
            .ok_or_else(|| format!("systemctl names no main process of {unit}: {text}"))?;
        if pid == 0 {
            return Err(format!("{unit} is not running"));
        }
        let process = rustix::fs::openat(
            CWD,
            format!("/proc/{pid}"),
            OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| format!("cannot open {unit}'s main process {pid}: {error}"))?;
        let runs_as = rustix::fs::fstat(&process)
            .map_err(|error| format!("cannot read {unit}'s main process {pid}: {error}"))?
            .st_uid;
        if runs_as != self.uid {
            return Err(format!(
                "{unit}'s main process {pid} runs as uid {runs_as}, not {}'s uid {}",
                self.user, self.uid
            ));
        }
        let namespace_of = |kind: &str| {
            let namespace = rustix::fs::openat(
                &process,
                format!("ns/{kind}"),
                OFlags::RDONLY | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|error| format!("cannot open {unit}'s {kind} namespace: {error}"))?;
            let theirs = rustix::fs::fstat(&namespace)
                .map_err(|error| format!("cannot read {unit}'s {kind} namespace: {error}"))?;
            let own = rustix::fs::stat(format!("/proc/self/ns/{kind}"))
                .map_err(|error| format!("cannot read the helper's {kind} namespace: {error}"))?;
            let is_own = (theirs.st_dev, theirs.st_ino) == (own.st_dev, own.st_ino);
            Ok::<_, String>((namespace, is_own))
        };
        let (_, is_own_users) = namespace_of("user")?;
        if !is_own_users {
            return Err(format!(
                "{unit}'s main process {pid} runs in a user namespace of its own"
            ));
        }
        let (namespace, is_own_mounts) = namespace_of("mnt")?;
        if is_own_mounts {
            return Err(format!("{unit} has no mount namespace of its own"));
        }
        Ok(Unit { process, namespace })
    }

    /// The unit must show the engine none of the host's folders the checkouts are in: the
    /// one the project's checkout is in, nor the clones folder. Folders the helper made
    /// there are the unit's own.
    fn check_unit_view(&self, unit: &Unit) -> Result<(), String> {
        let unit_root = rustix::fs::openat(
            &unit.process,
            "root",
            OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| format!("cannot open the engine's view: {error}"))?;
        let project_folder = self.project_root.parent().unwrap_or(&self.project_root);
        for folder in [project_folder, self.clones_dir.as_path()] {
            let on_host = rustix::fs::fstat(open_folder(folder)?)
                .map_err(|error| format!("cannot read {}: {error}", folder.display()))?;
            let Some(in_unit) = open_beneath(&unit_root, Path::new("/"), folder, Missing::Absent)?
            else {
                continue;
            };
            let in_unit = rustix::fs::fstat(in_unit).map_err(|error| {
                format!("cannot read the engine's {}: {error}", folder.display())
            })?;
            if (in_unit.st_dev, in_unit.st_ino) == (on_host.st_dev, on_host.st_ino) {
                return Err(format!(
                    "the engine's unit shows {} as the host has it, unmasked: run the \
                     engine's setup again",
                    folder.display()
                ));
            }
        }
        Ok(())
    }
}

/// The engine's unit, as its main process shows it.
struct Unit {
    /// The main process's folder in `/proc`.
    process: OwnedFd,
    namespace: OwnedFd,
}

/// Answers the one request on standard input. A refusal is answered and logged, and is no
/// failure of the helper's: only an answer that cannot be sent is.
pub fn serve(engines_dir: &Path) -> Result<(), String> {
    let mut stream = std::io::stdin()
        .as_fd()
        .try_clone_to_owned()
        .map(UnixStream::from)
        .map_err(|error| format!("cannot take the connection: {error}"))?;
    let outcome = answer(engines_dir, &mut stream);
    let line = match &outcome {
        Ok(()) => "ok\n".to_owned(),
        Err(Refusal { answer, log }) => {
            eprintln!("torlek-sandbox: refused: {log}");
            format!("refused: {}\n", answer.replace('\n', " "))
        }
    };
    stream
        .write_all(line.as_bytes())
        .map_err(|error| format!("cannot answer: {error}"))
}

/// Why a request is refused: what the asker is told, and what the helper's journal keeps.
struct Refusal {
    answer: String,
    log: String,
}

impl From<String> for Refusal {
    fn from(reason: String) -> Refusal {
        Refusal {
            answer: reason.clone(),
            log: reason,
        }
    }
}

impl From<&str> for Refusal {
    fn from(reason: &str) -> Refusal {
        Refusal::from(reason.to_owned())
    }
}

fn answer(engines_dir: &Path, stream: &mut UnixStream) -> Result<(), Refusal> {
    let peer = rustix::net::sockopt::socket_peercred(&*stream)
        .map_err(|error| format!("cannot tell who asks: {error}"))?
        .uid
        .as_raw();
    let request = read_request(stream)?;
    let (user, checkout) = request
        .iter()
        .position(|byte| *byte == b' ')
        .map(|space| (&request[..space], &request[space + 1..]))
        .ok_or("the request is not an engine's user, a space and a checkout")?;
    let user = std::str::from_utf8(user)
        .ok()
        .filter(|name| is_plain_name(name))
        .ok_or_else(|| {
            format!(
                "the engine's user {:?} is not a plain name",
                String::from_utf8_lossy(user)
            )
        })?;
    // Until the owner is known, nobody learns more of the engine's setup than this.
    let not_set_up = |log: String| Refusal {
        answer: format!("no engine of {user} is set up for uid {peer}"),
        log,
    };
    let engine = Engine::load(engines_dir, user).map_err(not_set_up)?;
    if peer != engine.owner_uid {
        return Err(not_set_up(format!(
            "uid {peer} asked for {user}'s engine; only the owner's uid {} may",
            engine.owner_uid
        )));
    }
    let (checkout, folder) = engine.checkout(checkout)?;
    // Before anything touches the checkout, and before the helper leaves the host's mount
    // namespace, which the unit's is compared to.
    let unit = engine.unit()?;
    engine.check_unit_view(&unit)?;
    let tree = masked_tree(&checkout, &folder, engine.owner_uid)?;
    Ok(attach(&tree, &unit.namespace, &checkout)?)
}

/// A user name as `useradd` takes one and a unit's instance can hold.
fn is_plain_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name
            .chars()
            .all(|letter| letter.is_ascii_lowercase() || letter.is_ascii_digit() || letter == '-')
}

/// The request, without its newline.
fn read_request(stream: &mut UnixStream) -> Result<Vec<u8>, String> {
    stream
        .set_read_timeout(Some(DEADLINE))
        .map_err(|error| format!("cannot read the request: {error}"))?;
    let mut request = Vec::new();
    let mut chunk = [0_u8; 512];
    loop {
        let end = request.iter().position(|byte| *byte == b'\n');
        if end.unwrap_or(request.len()) > MAX_REQUEST {
            return Err(format!("the request is longer than {MAX_REQUEST} bytes"));
        }
        if let Some(end) = end {
            if end + 1 != request.len() {
                return Err("the request holds more than one line".to_owned());
            }
            request.truncate(end);
            return Ok(request);
        }
        match stream.read(&mut chunk) {
            Ok(0) => return Err("the request ended before its newline".to_owned()),
            Ok(read) => request.extend_from_slice(&chunk[..read]),
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => return Err(format!("cannot read the request: {error}")),
        }
    }
}

/// Where the helper's namespace holds the checkout while it masks it: a folder of its own
/// there, which nobody else can reach to swap the checkout for another.
const STAGE: &str = "/tmp";

/// A FUSE file system can show different files at each look, and the owner can mount one.
const FUSE_SUPER_MAGIC: u32 = 0x6573_5546;

/// The checkout, masked as a session's sandbox masks it, as a detached tree: built in a
/// mount namespace of the helper's own, so the host never sees the masks. The owner's
/// processes, a session's agent among them, can change the checkout while the helper works:
/// the helper plans and mounts the masks on the checkout bound in a folder of its own,
/// reaching each file without following a link.
fn masked_tree(checkout: &Path, checked: &OwnedFd, owner_uid: u32) -> Result<OwnedFd, String> {
    let seen = rustix::fs::fstat(checked)
        .map_err(|error| format!("cannot read {}: {error}", checkout.display()))?;
    let file_system = rustix::fs::fstatfs(checked)
        .map_err(|error| format!("cannot read {}'s file system: {error}", checkout.display()))?;
    if u64::try_from(file_system.f_type).ok() == Some(u64::from(FUSE_SUPER_MAGIC)) {
        return Err(format!(
            "{} is on a FUSE file system, which can show other files after each look",
            checkout.display()
        ));
    }
    #[expect(
        deprecated,
        reason = "rustix deprecates `unshare` for `UnshareFlags::FILES`, which this does not \
                  pass; its replacement is `unsafe`, which this crate denies"
    )]
    rustix::thread::unshare(UnshareFlags::NEWNS)
        .map_err(|error| format!("cannot make the helper's mount namespace: {error}"))?;
    rustix::mount::mount_change(
        "/",
        MountPropagationFlags::REC | MountPropagationFlags::PRIVATE,
    )
    .map_err(|error| format!("cannot keep the helper's mounts its own: {error}"))?;
    // Opened again in the helper's namespace, which binds only its own mounts.
    let folder = open_folder(checkout)?;
    let found = rustix::fs::fstat(&folder)
        .map_err(|error| format!("cannot read {}: {error}", checkout.display()))?;
    if (found.st_dev, found.st_ino) != (seen.st_dev, seen.st_ino) {
        return Err(format!(
            "{} changed while it was checked",
            checkout.display()
        ));
    }
    // A mount inside would come along unmasked, or a mask cover it without what it holds;
    // Torlek's checkouts have none.
    if let Some(inside) = mount_points()?
        .into_iter()
        .find(|point| point != checkout && point.starts_with(checkout))
    {
        return Err(format!(
            "{} holds a mount at {}: no checkout with a mount inside is shown to the engine",
            checkout.display(),
            inside.display()
        ));
    }
    let (staged, stage) = stage_checkout(&folder)?;
    let CheckoutMasks { protected, hidden } = cockpit_sandbox::checkout_masks(&staged);
    let protection = plan::protect(&protected, std::slice::from_ref(&staged))?;
    let hidden = plan::hide(&hidden, |path| path.starts_with(&staged))?;
    let masks: Vec<&Mask> = protection.masks.iter().chain(&hidden).collect();
    let inside = Inside {
        folder: &stage,
        checkout: &staged,
    };
    for mask in &masks {
        apply(&inside, mask, owner_uid, seen.st_gid)?;
    }
    let mounted = mount_points()?;
    if let Some(missing) = masks
        .iter()
        .find(|mask| !mounted.iter().any(|point| point == mask.path()))
    {
        return Err(format!(
            "the mask of {} did not land there",
            missing.path().display()
        ));
    }
    plan::verify(&protection.verify)?;
    rustix::mount::open_tree(
        &stage,
        "",
        OpenTreeFlags::AT_EMPTY_PATH
            | OpenTreeFlags::OPEN_TREE_CLONE
            | OpenTreeFlags::OPEN_TREE_CLOEXEC
            | OpenTreeFlags::AT_RECURSIVE,
    )
    .map_err(|error| format!("cannot take {} masked: {error}", checkout.display()))
}

/// `folder` bound at a path only the helper's namespace has, and that bind opened: the
/// owner can rename the checkout away, but not the bind.
fn stage_checkout(folder: &OwnedFd) -> Result<(PathBuf, OwnedFd), String> {
    let stage = Path::new(STAGE);
    rustix::mount::mount(
        "tmpfs",
        path_of(&open_folder(stage)?),
        "tmpfs",
        MountFlags::NOSUID | MountFlags::NODEV,
        c"mode=0700",
    )
    .map_err(|error| format!("cannot make the helper's {STAGE}: {error}"))?;
    let staged = stage.join("checkout");
    let stage_folder = open_folder(stage)?;
    rustix::fs::mkdirat(&stage_folder, "checkout", Mode::from_raw_mode(0o700))
        .map_err(|error| format!("cannot make {}: {error}", staged.display()))?;
    let target = open_folder(&staged)?;
    rustix::mount::mount_bind_recursive(path_of(folder), path_of(&target))
        .map_err(|error| format!("cannot bind the checkout at {}: {error}", staged.display()))?;
    let bound = open_folder(&staged)?;
    Ok((staged, bound))
}

/// The staged checkout's folder and its path.
struct Inside<'a> {
    folder: &'a OwnedFd,
    checkout: &'a Path,
}

impl Inside<'_> {
    /// `path`, in the checkout, reached from its folder through the masks made so far,
    /// without following a link.
    fn open(&self, path: &Path) -> Result<OwnedFd, String> {
        open_beneath(self.folder, self.checkout, path, Missing::Absent)?
            .ok_or_else(|| format!("{} is gone", path.display()))
    }
}

/// The path the kernel resolves to the open file itself, through no name anyone can change.
fn path_of(file: &OwnedFd) -> PathBuf {
    PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()))
}

fn apply(inside: &Inside<'_>, mask: &Mask, owner_uid: u32, group: u32) -> Result<(), String> {
    let path = mask.path();
    let failed = |action: &str, error: rustix::io::Errno| {
        format!("cannot {action} {}: {error}", path.display())
    };
    match mask {
        Mask::Pinned(_) => {
            let file = inside.open(path)?;
            rustix::mount::mount_bind_recursive(path_of(&file), path_of(&file))
                .map_err(|error| failed("pin", error))
        }
        Mask::ReadOnly(_) => {
            let file = inside.open(path)?;
            rustix::mount::mount_bind_recursive(path_of(&file), path_of(&file))
                .map_err(|error| failed("bind", error))?;
            // Opened again: the earlier descriptor names what lies under the new bind.
            remount_read_only(&inside.open(path)?, path)
        }
        Mask::EmptyFolder(_) => {
            let (parent, name) = path
                .parent()
                .zip(path.file_name())
                .ok_or_else(|| format!("{} has no folder", path.display()))?;
            let parent = inside.open(parent)?;
            rustix::fs::mkdirat(&parent, name, Mode::from_raw_mode(0o755))
                .map_err(|error| failed("make", error))?;
            let made = rustix::fs::openat(
                &parent,
                name,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|error| failed("open", error))?;
            std::os::unix::fs::fchown(&made, Some(owner_uid), Some(group))
                .map_err(|error| format!("cannot give {} to the owner: {error}", path.display()))?;
            empty_folder(&made, c"mode=0555").map_err(|error| failed("empty", error))
        }
        Mask::HiddenFolder(_) => {
            empty_folder(&inside.open(path)?, c"mode=0000").map_err(|error| failed("hide", error))
        }
        Mask::HiddenFile(_) => {
            rustix::mount::mount_bind("/dev/null", path_of(&inside.open(path)?))
                .map_err(|error| failed("hide", error))?;
            remount_read_only(&inside.open(path)?, path)
        }
    }
}

fn empty_folder(folder: &OwnedFd, mode: &std::ffi::CStr) -> rustix::io::Result<()> {
    rustix::mount::mount(
        "tmpfs",
        path_of(folder),
        "tmpfs",
        MountFlags::RDONLY | MountFlags::NOSUID | MountFlags::NODEV,
        mode,
    )
}

/// The bind `mounted` read-only, keeping the flags it has: a user namespace cannot clear
/// those its parent set.
fn remount_read_only(mounted: &OwnedFd, path: &Path) -> Result<(), String> {
    let current = rustix::fs::fstatvfs(mounted)
        .map_err(|error| format!("cannot read {}'s mount: {error}", path.display()))?
        .f_flag;
    let kept = [
        (StatVfsMountFlags::NOSUID, MountFlags::NOSUID),
        (StatVfsMountFlags::NODEV, MountFlags::NODEV),
        (StatVfsMountFlags::NOEXEC, MountFlags::NOEXEC),
        (StatVfsMountFlags::NOATIME, MountFlags::NOATIME),
        (StatVfsMountFlags::NODIRATIME, MountFlags::NODIRATIME),
        (StatVfsMountFlags::RELATIME, MountFlags::RELATIME),
    ]
    .into_iter()
    .filter(|(found, _)| current.contains(*found))
    .fold(MountFlags::BIND | MountFlags::RDONLY, |flags, (_, flag)| {
        flags | flag
    });
    rustix::mount::mount_remount(path_of(mounted), kept, "")
        .map_err(|error| format!("cannot make {} read-only: {error}", path.display()))
}

/// Moves `tree` into the unit's mount namespace at the checkout's path, in place of the
/// tree an earlier request put there. The unit shows no checkout of its own
/// ([`Engine::check_unit_view`]), so between the earlier tree's removal and the new one's
/// arrival the engine finds only the folders the helper made there.
fn attach(tree: &OwnedFd, namespace: &OwnedFd, checkout: &Path) -> Result<(), String> {
    rustix::thread::move_into_link_name_space(namespace.as_fd(), Some(LinkNameSpaceType::Mount))
        .map_err(|error| format!("cannot enter the engine's mount namespace: {error}"))?;
    let root = open_root()?;
    open_beneath(&root, Path::new("/"), checkout, Missing::Made)?;
    while mount_points()?.iter().any(|point| point == checkout) {
        rustix::mount::unmount(checkout, UnmountFlags::DETACH | UnmountFlags::NOFOLLOW).map_err(
            |error| {
                format!(
                    "cannot take the engine's earlier view of {} away: {error}",
                    checkout.display()
                )
            },
        )?;
    }
    let target = open_folder(checkout)?;
    rustix::mount::move_mount(
        tree,
        "",
        &target,
        "",
        MoveMountFlags::MOVE_MOUNT_F_EMPTY_PATH | MoveMountFlags::MOVE_MOUNT_T_EMPTY_PATH,
    )
    .map_err(|error| {
        format!(
            "cannot show the engine {} masked: {error}",
            checkout.display()
        )
    })
}

fn required<'a>(key: &str, value: Option<&'a str>) -> Result<&'a str, String> {
    value.ok_or_else(|| format!("{key} is missing"))
}

fn parse_uid(key: &str, value: Option<&str>) -> Result<u32, String> {
    let text = required(key, value)?;
    text.parse()
        .map_err(|_| format!("{key} {text:?} is not a uid"))
}

fn parse_folder(key: &str, value: Option<&str>) -> Result<PathBuf, String> {
    plain_path(key, &OsString::from(required(key, value)?))
}

/// What [`open_beneath`] does with a folder that is missing.
#[derive(Clone, Copy, PartialEq)]
enum Missing {
    /// Nothing is opened.
    Absent,
    /// The helper makes it, as root's: the engine's user can write none of them.
    Made,
}

/// `path`, beneath `start` which is the folder `start_path`, each step opened from the last
/// without following a link and crossing the mounts there. `None` when a step is missing.
fn open_beneath(
    start: &OwnedFd,
    start_path: &Path,
    path: &Path,
    missing: Missing,
) -> Result<Option<OwnedFd>, String> {
    let relative = path
        .strip_prefix(start_path)
        .map_err(|_| format!("{} is outside {}", path.display(), start_path.display()))?;
    let names: Vec<_> = relative.components().collect();
    let mut current = rustix::fs::openat(
        start,
        ".",
        OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| format!("cannot open {}: {error}", start_path.display()))?;
    let mut reached = start_path.to_path_buf();
    for (index, component) in names.iter().enumerate() {
        let Component::Normal(name) = component else {
            return Err(format!("{} is not a plain path", path.display()));
        };
        reached.push(name);
        if missing == Missing::Made {
            match rustix::fs::mkdirat(&current, *name, Mode::from_raw_mode(0o755)) {
                Ok(()) | Err(rustix::io::Errno::EXIST) => {}
                Err(error) => return Err(format!("cannot make {}: {error}", reached.display())),
            }
        }
        let found = match rustix::fs::statat(&current, *name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(found) => found,
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(error) => return Err(format!("cannot read {}: {error}", reached.display())),
        };
        let file_type = FileType::from_raw_mode(found.st_mode);
        if file_type == FileType::Symlink {
            return Err(format!("{} is a symbolic link", reached.display()));
        }
        if index + 1 < names.len() && file_type != FileType::Directory {
            return Err(format!("{} is not a folder", reached.display()));
        }
        current = rustix::fs::openat(
            &current,
            *name,
            OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| format!("cannot open {}: {error}", reached.display()))?;
    }
    Ok(Some(current))
}

fn open_root() -> Result<OwnedFd, String> {
    rustix::fs::openat(
        CWD,
        "/",
        OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| format!("cannot open /: {error}"))
}

/// The folder at `path`, an absolute plain path, reached without following a link.
fn open_folder(path: &Path) -> Result<OwnedFd, String> {
    let folder = open_beneath(&open_root()?, Path::new("/"), path, Missing::Absent)?
        .ok_or_else(|| format!("{} does not exist", path.display()))?;
    let found = rustix::fs::fstat(&folder)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    if FileType::from_raw_mode(found.st_mode) != FileType::Directory {
        return Err(format!("{} is not a folder", path.display()));
    }
    Ok(folder)
}

/// Where each mount of the caller's mount namespace sits.
fn mount_points() -> Result<Vec<PathBuf>, String> {
    let table = std::fs::read("/proc/self/mountinfo")
        .map_err(|error| format!("cannot read the mount table: {error}"))?;
    Ok(table
        .split(|byte| *byte == b'\n')
        .filter_map(|line| line.split(|byte| *byte == b' ').nth(4))
        .map(|point| PathBuf::from(OsStr::from_bytes(&unescape(point))))
        .collect())
}

/// A mount table field without its octal escapes (`\040` for a space).
fn unescape(field: &[u8]) -> Vec<u8> {
    let mut text = Vec::with_capacity(field.len());
    let mut index = 0;
    while index < field.len() {
        let escaped = field
            .get(index + 1..index + 4)
            .filter(|digits| field[index] == b'\\' && digits.iter().all(u8::is_ascii_digit))
            .and_then(|digits| std::str::from_utf8(digits).ok())
            .and_then(|digits| u8::from_str_radix(digits, 8).ok());
        match escaped {
            Some(byte) => {
                text.push(byte);
                index += 4;
            }
            None => {
                text.push(field[index]);
                index += 1;
            }
        }
    }
    text
}

fn is_plain(path: &Path) -> bool {
    path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::RootDir | Component::Normal(_)))
        // `Path::components` drops a `.` and a trailing `/`; a plain path has neither.
        && path.as_os_str().as_bytes() == rebuild(path).as_os_str().as_bytes()
}

fn rebuild(path: &Path) -> PathBuf {
    path.components().collect()
}

fn plain_path(role: &str, text: &OsString) -> Result<PathBuf, String> {
    let path = PathBuf::from(text);
    if !is_plain(&path) || path.parent().is_none() {
        return Err(format!(
            "{role} {} is not a plain absolute path",
            path.display()
        ));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mount_table_field_loses_its_escapes() {
        assert_eq!(unescape(br"/home/a\040b\134c"), b"/home/a b\\c".to_vec());
        assert_eq!(unescape(br"/plain\04"), br"/plain\04".to_vec());
    }

    const ENGINE: &str = "# Written by Torlek's engine setup for project shop (#390).\n\
        uid=972\nowner_uid=1000\nproject_root=/home/theo/shop\n\
        clones_dir=/home/theo/.parallel/shop\n";

    #[test]
    fn an_engines_file_names_its_uids_and_checkouts() {
        let engine = Engine::parse("torlek-e3", ENGINE).unwrap();

        assert_eq!((engine.uid, engine.owner_uid), (972, 1000));
        assert_eq!(engine.project_root, Path::new("/home/theo/shop"));
        assert_eq!(engine.clones_dir, Path::new("/home/theo/.parallel/shop"));
        assert_eq!(engine.unit_name(), "torlek-engine@torlek-e3.service");
    }

    #[test]
    fn an_engines_file_with_a_key_missing_twice_or_unknown_is_refused() {
        for (text, reason) in [
            (ENGINE.replace("uid=972\n", ""), "uid is missing"),
            (format!("{ENGINE}uid=973\n"), "uid is set twice"),
            (format!("{ENGINE}network=host\n"), "unknown key"),
            (
                ENGINE.replace("/home/theo/shop", "shop"),
                "not a plain absolute path",
            ),
        ] {
            let error = Engine::parse("torlek-e3", &text).unwrap_err();

            assert!(error.contains(reason), "{reason}: {error}");
        }
    }

    #[test]
    fn an_engines_file_whose_clones_folder_and_checkout_nest_is_refused() {
        for (project_root, clones_dir) in [
            ("/home/theo/shop", "/home/theo/shop/.parallel"),
            (
                "/home/theo/.parallel/shop/main",
                "/home/theo/.parallel/shop",
            ),
        ] {
            let text = format!(
                "uid=972\nowner_uid=1000\nproject_root={project_root}\nclones_dir={clones_dir}\n"
            );

            let refusal = Engine::parse("torlek-e3", &text).unwrap_err();

            assert!(refusal.contains("hold one another"), "{refusal}");
        }
    }

    #[test]
    fn a_path_with_a_dot_or_a_trailing_slash_is_not_plain() {
        for path in ["/home/./shop", "/home/shop/", "home/shop", "/home/../shop"] {
            assert!(!is_plain(Path::new(path)), "{path}");
        }
        assert!(is_plain(Path::new("/home/shop")));
    }
}
