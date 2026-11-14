//! The root part of an approved run (#1387, the owner's option B of 2026-11-14): what the
//! daemon asks `torlek-sandbox root-run` for, and the floor of limits that helper keeps
//! whatever it is asked.
//!
//! The daemon runs the helper through `/usr/bin/sudo -A -k`, so the owner types their
//! password for every root part. The helper runs as root; a request names the command, its
//! working folder, the folders it writes and more folders it cannot see. The helper starts the
//! command in a system unit of its own with [`RootFloor::unit_properties`]: the request can add
//! limits but cannot lower the floor. The floor holds against a daemon bug, not against
//! someone holding the owner's password, who has `sudo` anyway.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The helper's verb: `torlek-sandbox root-run`.
pub const ROOT_RUN_VERB: &str = "root-run";

/// The longest request the helper reads, its newline included.
pub const MAX_ROOT_RUN_REQUEST_BYTES: usize = 64 * 1024;

/// The most folders a request may name in each list.
pub const MAX_ROOT_RUN_FOLDERS: usize = 64;

/// The byte the daemon sends with the run's standard input, output and error, ahead of the
/// request's line.
pub const ROOT_RUN_DESCRIPTORS_BYTE: u8 = b'R';

/// The owner's secrets under their home, which neither a confined run nor its root part reads
/// (#1387): keys, the credentials of Git, GitHub's CLI, cloud and container tools, package
/// registries and agent providers, keyrings and browser profiles. A list cannot name every
/// tool's secret: the guide says so.
pub const OWNER_SECRETS: &[&str] = &[
    ".ssh",
    ".gnupg",
    ".config/gh",
    ".netrc",
    ".git-credentials",
    ".config/git/credentials",
    ".aws",
    ".azure",
    ".config/gcloud",
    ".docker",
    ".kube",
    ".password-store",
    ".local/share/password-store",
    ".npmrc",
    ".pypirc",
    ".cargo/credentials",
    ".cargo/credentials.toml",
    ".claude/.credentials.json",
    ".codex/auth.json",
    ".local/share/keyrings",
    ".local/share/kwalletd",
    ".mozilla",
    ".config/google-chrome",
    ".config/chromium",
    ".config/BraveSoftware",
    ".config/vivaldi",
    ".config/microsoft-edge",
];

/// What a root part never sees: root's home and the password hashes.
const ROOT_SECRETS: &[&str] = &[
    "/root",
    "/etc/shadow",
    "/etc/shadow-",
    "/etc/gshadow",
    "/etc/gshadow-",
];

/// Torlek's own folders under the owner's home, as the default layout puts them: its tokens
/// and its state. The helper hides each `torlek*` folder it finds there too, a profile's.
const OWNER_TORLEK: &[&str] = &[".config/torlek", ".local/state/torlek"];

/// What a root part never writes, even inside a folder it may: the boot files, what grants
/// root or runs as root at boot, login or a schedule, and Torlek's own install, so no root
/// part leaves behind what would run later outside the floor. A list cannot name every such
/// file; [`ROOT_NEVER_HOLDS`] refuses the folders that hold them as a whole.
pub const ROOT_NEVER_WRITES: &[&str] = &[
    "/boot",
    "/efi",
    "/etc/sudoers",
    "/etc/sudoers.d",
    "/etc/passwd",
    "/etc/group",
    "/etc/pam.d",
    "/etc/security",
    "/etc/polkit-1",
    "/etc/systemd",
    "/usr/lib/systemd",
    "/etc/udev",
    "/usr/lib/udev",
    "/etc/crontab",
    "/etc/cron.d",
    "/var/spool/cron",
    "/etc/ld.so.preload",
    "/etc/ld.so.conf",
    "/etc/ld.so.conf.d",
    "/etc/profile",
    "/etc/profile.d",
    "/etc/environment",
    "/etc/torlek",
    "/usr/lib/torlek",
];

/// The folders a root part never writes as a whole: the system's programs, libraries and
/// configuration, and every home. A folder inside one may be written (`/etc/nginx`,
/// `/usr/local`, a checkout), never one of these or one that holds them.
pub const ROOT_NEVER_HOLDS: &[&str] = &[
    "/bin",
    "/sbin",
    "/lib",
    "/lib64",
    "/etc",
    "/usr",
    "/usr/bin",
    "/usr/sbin",
    "/usr/lib",
    "/usr/lib64",
    "/usr/libexec",
    "/var",
    "/var/lib",
    "/home",
];

/// The oldest systemd whose units take the floor's properties: `PrivatePIDs=` came in 257.
pub const ROOT_RUN_MIN_SYSTEMD: u32 = 257;

/// The system's runtime folder, emptied: its sockets and buses (systemd's, D-Bus, a container
/// engine's) would start what is not confined.
const ROOT_EMPTIED: &str = "/run";

/// The sandboxing every root part's unit gets, whatever it asked.
const FLOOR_PROPERTIES: &[&str] = &[
    "ProtectSystem=strict",
    "PrivateTmp=yes",
    "PrivateDevices=yes",
    "PrivateNetwork=yes",
    "PrivateIPC=yes",
    // A process namespace of its own and its `/proc`: root's processes outside are root's
    // too, and `/proc/PID/root` of one would reach the host's files past every mount here.
    "PrivatePIDs=yes",
    "ProtectKernelTunables=yes",
    "ProtectKernelModules=yes",
    "ProtectKernelLogs=yes",
    "ProtectControlGroups=yes",
    "ProtectClock=yes",
    "ProtectHostname=yes",
    "RestrictNamespaces=yes",
    "RestrictRealtime=yes",
    "LockPersonality=yes",
    "NoNewPrivileges=yes",
    // No setuid, setgid or file-capability program left behind: it would run as root later,
    // outside every limit here.
    "RestrictSUIDSGID=yes",
    "KeyringMode=private",
    // What would undo the mounts above (`CAP_SYS_ADMIN`, `CAP_DAC_READ_SEARCH` with
    // `open_by_handle_at`), load code into the kernel, reach devices or other processes, or
    // make a file root cannot change back or one that grants capabilities.
    "CapabilityBoundingSet=~CAP_SYS_ADMIN CAP_DAC_READ_SEARCH CAP_SYS_MODULE CAP_SYS_PTRACE \
     CAP_SYS_RAWIO CAP_SYS_BOOT CAP_SYS_TIME CAP_BPF CAP_PERFMON CAP_NET_ADMIN CAP_MAC_ADMIN \
     CAP_MAC_OVERRIDE CAP_MKNOD CAP_LINUX_IMMUTABLE CAP_SETFCAP CAP_SYSLOG CAP_AUDIT_CONTROL",
    "SystemCallFilter=~@mount @module @reboot @swap @raw-io @debug @obsolete @cpu-emulation \
     @clock open_by_handle_at",
];

/// What the daemon asks the helper to run as root, for one `sudo` of one approved run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootRunRequest {
    /// The approved run, for the unit's name.
    pub run: u64,
    /// The run's `sudo` this is, counted from 1, for the unit's name.
    pub ask: u64,
    /// The command: an absolute program, then its arguments.
    pub argv: Vec<String>,
    /// The folder it starts in.
    pub cwd: PathBuf,
    /// The folders it writes; everything else is read-only.
    pub writable: Vec<PathBuf>,
    /// More folders it cannot see, beside the floor's.
    pub hidden: Vec<PathBuf>,
    /// The run's `TERM`, if it has one.
    pub term: Option<String>,
}

/// The limits no request lowers: the owner's secrets, Torlek's folders and root's hidden;
/// [`ROOT_NEVER_WRITES`] read-only; [`ROOT_NEVER_HOLDS`] and the owner's home never written as a
/// whole; `/run` emptied; and the sandboxing every root part gets (`FLOOR_PROPERTIES`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootFloor {
    hidden: Vec<PathBuf>,
    read_only: Vec<PathBuf>,
    never_held: Vec<PathBuf>,
}

impl RootFloor {
    /// The floor of the owner whose home is `owner_home`.
    pub fn for_owner(owner_home: &Path) -> RootFloor {
        RootFloor {
            hidden: OWNER_SECRETS
                .iter()
                .chain(OWNER_TORLEK)
                .map(|secret| owner_home.join(secret))
                .chain(ROOT_SECRETS.iter().map(PathBuf::from))
                .collect(),
            read_only: ROOT_NEVER_WRITES.iter().map(PathBuf::from).collect(),
            never_held: ROOT_NEVER_HOLDS
                .iter()
                .map(PathBuf::from)
                .chain([owner_home.to_path_buf()])
                .collect(),
        }
    }

    /// The floor hiding `paths` too.
    pub fn hiding(mut self, paths: impl IntoIterator<Item = PathBuf>) -> RootFloor {
        self.hidden.extend(paths);
        self
    }

    /// The floor keeping `paths` read-only too.
    pub fn keeping(mut self, paths: impl IntoIterator<Item = PathBuf>) -> RootFloor {
        self.read_only.extend(paths);
        self
    }

    /// The floor with each of its paths also as `resolve` gives it, so a folder reached
    /// through a symbolic link is still the floor's.
    pub fn resolved(mut self, resolve: impl Fn(&Path) -> Option<PathBuf>) -> RootFloor {
        for paths in [&mut self.hidden, &mut self.read_only, &mut self.never_held] {
            let resolved: Vec<PathBuf> = paths
                .iter()
                .filter_map(|path| resolve(path).filter(|resolved| resolved != path))
                .collect();
            paths.extend(resolved);
        }
        self
    }

    /// The unit's properties for `request`, or why the helper refuses it. A folder the
    /// request writes is refused when it is or lies inside one the floor keeps or hides, or
    /// when it is or holds one the floor never lets be written as a whole, or a secret the
    /// floor hides, present or not. One that holds a folder the floor keeps read-only is
    /// allowed, and that folder stays read-only inside it.
    pub fn unit_properties(&self, request: &RootRunRequest) -> Result<Vec<String>, String> {
        check_command(request)?;
        if request.writable.len() > MAX_ROOT_RUN_FOLDERS
            || request.hidden.len() > MAX_ROOT_RUN_FOLDERS
        {
            return Err(format!(
                "a root part names at most {MAX_ROOT_RUN_FOLDERS} folders it writes and \
                 {MAX_ROOT_RUN_FOLDERS} it cannot see"
            ));
        }
        let mut properties: Vec<String> =
            FLOOR_PROPERTIES.iter().map(|p| (*p).to_owned()).collect();
        properties.push(format!("TemporaryFileSystem={ROOT_EMPTIED}:ro"));
        for path in self.hidden.iter().chain(&request.hidden) {
            properties.push(format!("InaccessiblePaths=-\"{}\"", systemd_path(path)?));
        }
        for path in &self.read_only {
            properties.push(format!("ReadOnlyPaths=-\"{}\"", systemd_path(path)?));
        }
        for path in &request.writable {
            let text = systemd_path(path)?;
            self.check_writable(path, &request.hidden)?;
            properties.push(format!("ReadWritePaths=\"{text}\""));
        }
        Ok(properties)
    }

    fn check_writable(&self, path: &Path, also_hidden: &[PathBuf]) -> Result<(), String> {
        let shown = path.display();
        if path == Path::new("/") {
            return Err("a root part cannot write the whole file system".to_owned());
        }
        if let Some(kept) = self.read_only.iter().find(|kept| path.starts_with(kept)) {
            return Err(format!(
                "a root part never writes {shown}: {} is kept read-only",
                kept.display()
            ));
        }
        if let Some(whole) = self.never_held.iter().find(|whole| whole.starts_with(path)) {
            return Err(format!(
                "a root part never writes {shown}: it would write all of {}; name a folder \
                 inside it",
                whole.display()
            ));
        }
        let emptied = PathBuf::from(ROOT_EMPTIED);
        if let Some(hidden) = self
            .hidden
            .iter()
            .chain(also_hidden)
            .chain([&emptied])
            .find(|hidden| path.starts_with(hidden))
        {
            return Err(format!(
                "a root part cannot write {shown}: it cannot see {}",
                hidden.display()
            ));
        }
        if let Some(secret) = self.hidden.iter().find(|secret| secret.starts_with(path)) {
            return Err(format!(
                "a root part never writes {shown}: it holds {}, which stays hidden",
                secret.display()
            ));
        }
        Ok(())
    }
}

/// The program, its working folder and its terminal's name, as the unit takes them.
fn check_command(request: &RootRunRequest) -> Result<(), String> {
    let Some(program) = request.argv.first() else {
        return Err("a root part names no program".to_owned());
    };
    if !Path::new(program).is_absolute() {
        return Err(format!(
            "a root part's program must be absolute, not {program:?}"
        ));
    }
    if let Some(argument) = request.argv.iter().find(|argument| argument.contains('\0')) {
        return Err(format!(
            "a root part's argument holds a NUL byte: {argument:?}"
        ));
    }
    systemd_path(&request.cwd)?;
    if let Some(term) = &request.term {
        let is_name = !term.is_empty()
            && term.len() <= 64
            && term
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._+-".contains(&byte));
        if !is_name {
            return Err(format!("{term:?} is not a terminal's name"));
        }
    }
    Ok(())
}

/// `path` as a systemd property may hold it: absolute, UTF-8, and without what its parser
/// gives a meaning (quotes, escapes, specifiers, the bind separator `:`) or a control
/// character.
fn systemd_path(path: &Path) -> Result<&str, String> {
    let refused = |why: &str| format!("{} {why}", path.display());
    let text = path.to_str().ok_or_else(|| refused("is not UTF-8"))?;
    if !path.is_absolute() {
        return Err(refused("is not absolute"));
    }
    if let Some(character) = text.chars().find(|character| {
        matches!(character, '"' | '\'' | '\\' | '%' | ':') || character.is_control()
    }) {
        return Err(refused(&format!(
            "holds {character:?}, which systemd would read as more than a path"
        )));
    }
    Ok(text)
}

/// The unit of `request` for the owner `owner_uid`, unique to the helper's process too, so two
/// daemons' runs never share one.
pub fn root_run_unit(owner_uid: u32, request: &RootRunRequest, helper_pid: u32) -> String {
    format!(
        "torlek-root-run-{owner_uid}-{}-{}-{helper_pid}.service",
        request.run, request.ask
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(writable: &[&str]) -> RootRunRequest {
        RootRunRequest {
            run: 4,
            ask: 1,
            argv: vec!["/usr/bin/make".to_owned(), "install".to_owned()],
            cwd: PathBuf::from("/home/owner/shop"),
            writable: writable.iter().map(PathBuf::from).collect(),
            hidden: vec![PathBuf::from("/srv/more-hidden")],
            term: Some("xterm-256color".to_owned()),
        }
    }

    fn floor() -> RootFloor {
        RootFloor::for_owner(Path::new("/home/owner"))
    }

    #[test]
    fn every_root_part_gets_the_floors_sandboxing_whatever_it_asks() {
        let properties = floor().unit_properties(&request(&["/usr/local"])).unwrap();
        for kept in FLOOR_PROPERTIES {
            assert!(properties.iter().any(|property| property == kept), "{kept}");
        }
        assert!(properties.contains(&"TemporaryFileSystem=/run:ro".to_owned()));
    }

    #[test]
    fn the_owners_keys_and_roots_secrets_are_hidden_beside_what_the_request_hides() {
        let properties = floor().unit_properties(&request(&[])).unwrap();
        for hidden in [
            "/home/owner/.ssh",
            "/home/owner/.gnupg",
            "/root",
            "/etc/shadow",
            "/home/owner/.config/torlek",
            "/home/owner/.local/state/torlek",
            "/srv/more-hidden",
        ] {
            let property = format!("InaccessiblePaths=-\"{hidden}\"");
            assert!(properties.contains(&property), "{property}");
        }
    }

    #[test]
    fn a_folder_inside_etc_is_written_while_what_grants_root_stays_read_only() {
        let properties = floor()
            .unit_properties(&request(&["/etc/nginx", "/usr/local"]))
            .unwrap();
        assert!(properties.contains(&"ReadWritePaths=\"/etc/nginx\"".to_owned()));
        for kept in [
            "/etc/sudoers",
            "/etc/passwd",
            "/etc/systemd",
            "/usr/lib/torlek",
            "/boot",
        ] {
            let property = format!("ReadOnlyPaths=-\"{kept}\"");
            assert!(properties.contains(&property), "{property}");
        }
    }

    #[test]
    fn a_root_part_that_asks_to_write_what_the_floor_keeps_is_refused() {
        for kept in [
            "/boot",
            "/boot/grub",
            "/etc/sudoers.d/agent",
            "/etc/passwd",
            "/etc/systemd/system",
            "/usr/lib/systemd/system",
            "/usr/lib/torlek/bin",
        ] {
            let refused = floor().unit_properties(&request(&[kept])).unwrap_err();
            assert!(refused.contains("is kept read-only"), "{kept}: {refused}");
        }
    }

    #[test]
    fn a_root_part_never_writes_a_system_folder_or_a_home_as_a_whole() {
        for whole in ["/etc", "/usr", "/usr/lib", "/var", "/home", "/home/owner"] {
            let refused = floor().unit_properties(&request(&[whole])).unwrap_err();
            assert!(refused.contains("would write all of"), "{whole}: {refused}");
        }
    }

    #[test]
    fn a_root_part_never_writes_a_folder_holding_a_secret_even_one_not_made_yet() {
        let refused = floor()
            .unit_properties(&request(&["/home/owner/.config"]))
            .unwrap_err();
        assert!(refused.contains("stays hidden"), "{refused}");
    }

    #[test]
    fn what_the_helper_keeps_read_only_cannot_be_written() {
        let refused = floor()
            .keeping([PathBuf::from("/opt/torlek/bin/torlek-sandbox")])
            .unit_properties(&request(&["/opt/torlek/bin/torlek-sandbox"]))
            .unwrap_err();
        assert!(refused.contains("is kept read-only"), "{refused}");
    }

    #[test]
    fn a_root_part_cannot_write_what_it_cannot_see() {
        for hidden in [
            "/home/owner/.ssh",
            "/root/.config",
            "/run/dbus",
            "/home/owner/.local/state/torlek/db",
            "/srv/more-hidden/x",
        ] {
            let refused = floor().unit_properties(&request(&[hidden])).unwrap_err();
            assert!(refused.contains("cannot see"), "{hidden}: {refused}");
        }
    }

    #[test]
    fn a_root_part_cannot_write_the_whole_file_system() {
        let refused = floor().unit_properties(&request(&["/"])).unwrap_err();
        assert!(refused.contains("whole file system"), "{refused}");
    }

    #[test]
    fn a_floor_folder_reached_through_a_link_is_still_kept() {
        let resolved = floor().resolved(|path| {
            (path == Path::new("/boot")).then(|| PathBuf::from("/mnt/boot-partition"))
        });
        let refused = resolved
            .unit_properties(&request(&["/mnt/boot-partition/grub"]))
            .unwrap_err();
        assert!(refused.contains("never writes"), "{refused}");
    }

    #[test]
    fn a_relative_program_is_refused() {
        let mut asked = request(&[]);
        asked.argv[0] = "make".to_owned();
        let refused = floor().unit_properties(&asked).unwrap_err();
        assert!(refused.contains("must be absolute"), "{refused}");
    }

    #[test]
    fn an_empty_command_is_refused() {
        let mut asked = request(&[]);
        asked.argv.clear();
        let refused = floor().unit_properties(&asked).unwrap_err();
        assert!(refused.contains("names no program"), "{refused}");
    }

    #[test]
    fn a_path_systemd_would_read_as_more_is_refused() {
        for path in ["/srv/a:b", "/srv/%h", "/srv/\"x", "relative"] {
            let refused = floor().unit_properties(&request(&[path])).unwrap_err();
            assert!(!refused.is_empty(), "{path}");
        }
    }

    #[test]
    fn a_term_that_is_no_terminal_name_is_refused() {
        let mut asked = request(&[]);
        asked.term = Some("xterm\nExecStart=/bin/sh".to_owned());
        let refused = floor().unit_properties(&asked).unwrap_err();
        assert!(refused.contains("terminal's name"), "{refused}");
    }

    #[test]
    fn too_many_folders_are_refused() {
        let many: Vec<String> = (0..=MAX_ROOT_RUN_FOLDERS)
            .map(|n| format!("/srv/{n}"))
            .collect();
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        let refused = floor().unit_properties(&request(&many)).unwrap_err();
        assert!(refused.contains("at most"), "{refused}");
    }

    #[test]
    fn a_request_with_an_unknown_field_does_not_parse() {
        let text = r#"{"run":1,"ask":1,"argv":["/bin/true"],"cwd":"/","writable":[],"hidden":[],"term":null,"properties":["PrivateNetwork=no"]}"#;
        assert!(serde_json::from_str::<RootRunRequest>(text).is_err());
    }

    #[test]
    fn each_root_part_has_a_unit_of_its_own() {
        assert_eq!(
            root_run_unit(1000, &request(&[]), 77),
            "torlek-root-run-1000-4-1-77.service"
        );
    }
}
