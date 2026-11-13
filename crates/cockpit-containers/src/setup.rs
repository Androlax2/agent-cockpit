//! The guided root step that sets up a project's engine (#390, #856 Q2): a script `torlek
//! doctor` writes for the owner to review and run once with `sudo`. Torlek never runs it.
//!
//! It makes the engine's system user and its subordinate ids, lets that user write the
//! project's checkouts with ACLs whose default entries keep what containers create there the
//! owner's to change and delete, installs the `torlek-engine@` unit, whose `/home` is empty,
//! and restarts the engine. The unit has no network until the engine's egress proxy exists:
//! pulls fail closed. It records the engine for Torlek's masks helper (`ENGINES_DIR`), which
//! shows the engine each checkout a session asks for through that session's masks, and lets
//! only the owner reach the helper's socket.
//!
//! Beside it runs `torlek-engine-relay@`, in the engine's network namespace: the owner's
//! installed `torlek-sandbox engine-relay`, bound read-only, which relays the owner to the
//! ports the engine published. Whatever can write that installed program runs as the engine
//! user; Theo chose that over a root-owned copy to refresh after each upgrade.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::{Account, ENGINES_DIR, EngineUser, MASKS_SOCKET_UNIT};

/// Folders both units make inaccessible.
const HIDDEN_FOLDERS: [&str; 7] = [
    "/root", "/mnt", "/media", "/srv", "/opt", "/var/log", "/boot",
];

/// What the script sets up.
#[derive(Debug, Clone, Copy)]
pub struct SetupPlan<'a> {
    pub project: &'a str,
    pub user: &'a EngineUser,
    /// The daemon's user: it keeps every file in the checkouts and reaches the socket.
    pub owner: &'a Account,
    /// The project's main checkout.
    pub project_root: &'a Path,
    /// Where the project's parallel clones live, made if missing.
    pub clones_dir: &'a Path,
    /// The command that records the engine once it runs: `torlek containers record NAME`.
    pub record_command: &'a str,
    /// The installed `torlek-sandbox`, which the relay unit runs as the engine's user.
    pub relay_program: &'a Path,
}

/// The script, or why these paths cannot be set up: each must be absolute, inside the
/// owner's home, and plain enough to write unquoted in a unit file.
pub fn setup_script(plan: &SetupPlan<'_>) -> Result<String, String> {
    let home = &plan.owner.home;
    if !home.is_absolute() || home.parent().is_none() {
        return Err(format!(
            "{}'s home {} cannot be hidden from the engine",
            plan.owner.name,
            home.display()
        ));
    }
    let root = unit_path("the project's checkout", plan.project_root)?;
    let clones = unit_path("the project's clones folder", plan.clones_dir)?;
    let home_text = unit_path("your home", home)?;
    let relay_program = unit_path("the sandbox launcher", plan.relay_program)?;
    if let Some(hidden) = HIDDEN_FOLDERS
        .iter()
        .find(|folder| plan.relay_program.starts_with(folder))
    {
        return Err(format!(
            "the sandbox launcher {relay_program} is under {hidden}, which the engine's units \
             hide: install Torlek elsewhere"
        ));
    }
    let inaccessible = HIDDEN_FOLDERS
        .iter()
        .map(|folder| format!("-{folder}"))
        .collect::<Vec<_>>()
        .join(" ");
    let relay_socket = plan.user.relay_socket();
    let relay_socket = unit_path("the relay's socket", &relay_socket)?;
    // The helper refuses them: showing the engine one would take the other's trees away.
    if plan.project_root.starts_with(plan.clones_dir)
        || plan.clones_dir.starts_with(plan.project_root)
    {
        return Err(format!(
            "the project's checkout {root} and its clones folder {clones} are inside one \
             another: the engine sees them only apart"
        ));
    }
    for (role, path) in [
        ("the project's checkout", plan.project_root),
        ("the project's clones folder", plan.clones_dir),
    ] {
        if !path.starts_with(home) || path == home {
            return Err(format!(
                "{role} {} is not inside your home {home_text}: the engine sees only checkouts \
                 there",
                path.display()
            ));
        }
    }
    if !is_plain(plan.owner.name.as_str()) {
        return Err(format!("the user name {:?} is not plain", plan.owner.name));
    }
    if !plan
        .record_command
        .chars()
        .all(|letter| letter.is_ascii_alphanumeric() || matches!(letter, ' ' | '-' | '_' | '.'))
    {
        return Err(format!(
            "the command {:?} is not plain",
            plan.record_command
        ));
    }
    // Under /home, the units' empty /home hides it already. The engine's stays writable to
    // root, where the masks helper makes the folders above each checkout it shows.
    let (hide_home_engine, hide_home_relay) = if home.starts_with("/home") {
        (String::new(), String::new())
    } else {
        (
            format!("TemporaryFileSystem={home_text}\n"),
            format!("TemporaryFileSystem={home_text}:ro\n"),
        )
    };
    Ok(SCRIPT
        .replace("{project}", plan.project)
        .replace("{user}", plan.user.name())
        .replace("{owner}", &plan.owner.name)
        .replace("{project_root}", &root)
        .replace("{clones_dir}", &clones)
        .replace("{hide_home_engine}", &hide_home_engine)
        .replace("{hide_home_relay}", &hide_home_relay)
        .replace("{engines_dir}", ENGINES_DIR)
        .replace("{masks_socket_unit}", MASKS_SOCKET_UNIT)
        .replace("{relay_program}", &relay_program)
        .replace("{relay_socket}", &relay_socket)
        .replace("{owner_uid}", &plan.owner.uid.to_string())
        .replace("{inaccessible}", &inaccessible)
        .replace("{record}", plan.record_command))
}

/// Writes `script` as `<dir>/<user>-setup.sh`, readable by the owner alone, replacing the
/// one written before in one step. Returns its path.
pub fn write_setup_script(dir: &Path, user: &EngineUser, script: &str) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir)
        .map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
    let path = dir.join(format!("{}-setup.sh", user.name()));
    // Two reports may write the same script at once, from the same daemon process.
    static WRITES: AtomicU64 = AtomicU64::new(0);
    let partial = dir.join(format!(
        ".{}-setup.sh.{}.{}",
        user.name(),
        std::process::id(),
        WRITES.fetch_add(1, Ordering::Relaxed)
    ));
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&partial)
        .and_then(|mut file| {
            file.write_all(script.as_bytes())?;
            file.sync_all()
        })
        .and_then(|()| std::fs::rename(&partial, &path));
    if let Err(error) = written {
        let _ = std::fs::remove_file(&partial);
        return Err(format!("cannot write {}: {error}", path.display()));
    }
    Ok(path)
}

/// `path` as the script and the helper's engine file take it: absolute UTF-8 without `..`
/// (which would escape the home check), whitespace, quotes, backslashes or `$`. `%` and `:`
/// stay refused, as when the unit bound these folders.
fn unit_path(role: &str, path: &Path) -> Result<String, String> {
    let shown = path.display();
    let text = path
        .to_str()
        .ok_or_else(|| format!("{role} {shown} is not UTF-8"))?;
    if !path.is_absolute() {
        return Err(format!("{role} {shown} is not absolute"));
    }
    let is_normal = path
        .components()
        .all(|component| matches!(component, Component::RootDir | Component::Normal(_)));
    if !is_normal {
        return Err(format!("{role} {shown} holds `..`"));
    }
    if !is_plain(text) {
        return Err(format!(
            "{role} {shown} holds a space, a quote, a backslash or one of `$%:`, which the \
             engine's setup cannot take"
        ));
    }
    Ok(text.to_owned())
}

fn is_plain(text: &str) -> bool {
    !text.is_empty()
        && !text.chars().any(|letter| {
            letter.is_whitespace()
                || letter.is_control()
                || matches!(letter, '\'' | '"' | '\\' | '$' | '%' | ':' | '`')
        })
}

const SCRIPT: &str = r#"#!/bin/sh
# Torlek: the rootless Docker engine of project {project} (#390), written by `torlek doctor`.
# Review it, then run it once with sudo. It makes the engine's user {user}, lets that user
# write the project's checkouts, installs the units torlek-engine@{user}.service and
# torlek-engine-relay@{user}.service and starts them. Running it again is safe: it rewrites
# the units and restarts the engine. Run it again if Torlek's programs move.
set -eu

user='{user}'
owner='{owner}'
project_root='{project_root}'
clones_dir='{clones_dir}'
relay_program='{relay_program}'
unit_dir=/etc/systemd/system

fail() {
    echo "torlek engine setup: $*" >&2
    exit 1
}

[ "$(id -u)" = 0 ] || fail "run it with sudo"
for program in dockerd-rootless.sh rootlesskit slirp4netns newuidmap newgidmap setfacl \
    useradd usermod runuser systemctl; do
    command -v "$program" >/dev/null 2>&1 ||
        fail "$program is missing: install rootless Docker (dockerd-rootless.sh, rootlesskit, slirp4netns), newuidmap and newgidmap, and setfacl"
done
[ -d "$project_root" ] || fail "no folder $project_root"
[ -f "$relay_program" ] || fail "no program $relay_program: install Torlek first"
# The relay runs it as $user, not as root.
[ -n "$(find "$relay_program" -maxdepth 0 -perm -0001)" ] ||
    fail "$user cannot run $relay_program: make it executable by others (chmod o+rx)"
dockerd_rootless=$(command -v dockerd-rootless.sh)
setfacl_program=$(command -v setfacl)
nologin=$(command -v nologin || echo /bin/false)
# Torlek's masks helper lets one user connect: a second owner's setup would take it.
masks_owner="$unit_dir/{masks_socket_unit}.d/owner.conf"
[ ! -f "$masks_owner" ] || grep -qx "SocketUser=$owner" "$masks_owner" ||
    fail "Torlek's masks helper serves another user on this host ($(grep '^SocketUser=' "$masks_owner")); it serves one owner"

if ! id -u "$user" >/dev/null 2>&1; then
    useradd --system --user-group --create-home --home-dir "/var/lib/$user" \
        --shell "$nologin" "$user"
fi
# Each engine's images and containers stay its own, whatever mode useradd gave the home.
chmod 0700 "/var/lib/$user"

# Rootless Docker maps its containers' users onto 65,536 subordinate ids of its own, after
# every range already given out.
next_range() {
    start=$(cat /etc/subuid /etc/subgid 2>/dev/null |
        awk -F: '{ end = $2 + $3; if (end > max) max = end } END { print (max < 200000 ? 200000 : max) }')
    echo "$start-$((start + 65535))"
}
# One range for both, taken before either is added, so the uids and gids match.
range=$(next_range)
grep -q "^$user:" /etc/subuid 2>/dev/null || usermod --add-subuids "$range" "$user"
grep -q "^$user:" /etc/subgid 2>/dev/null || usermod --add-subgids "$range" "$user"

# The engine's user writes the checkouts; what its containers create there stays yours to
# change and delete (the default entries).
runuser -u "$owner" -- mkdir -p "$clones_dir"
setfacl -R -m "u:$user:rwX,d:u:$user:rwX,d:u:$owner:rwX" "$project_root" "$clones_dir"

cat >"$unit_dir/torlek-engine@.service" <<UNIT
# Written by Torlek's engine setup (#390); running a setup again rewrites it.
[Unit]
Description=Torlek's rootless Docker engine for %i

[Service]
User=%i
Group=%i
Environment=HOME=/var/lib/%i
Environment=XDG_RUNTIME_DIR=/run/%i
RuntimeDirectory=%i
RuntimeDirectoryMode=0700
ExecStart=$dockerd_rootless --exec-opt native.cgroupdriver=cgroupfs
# No route out until Torlek's egress proxy serves the engine: pulls fail closed.
PrivateNetwork=yes
PrivateTmp=yes
# No home and no checkout: Torlek's masks helper shows the engine each checkout a session
# asks for, through that session's masks. The empty /home is root's, so the helper makes
# the folders above a checkout there and the engine's user none.
TemporaryFileSystem=/home
# The helper's later mounts reach rootlesskit's namespace below this one only so.
MountFlags=shared
InaccessiblePaths={inaccessible}
Delegate=yes
KillMode=mixed
Restart=on-failure
RestartSec=2
# Its relay runs whenever it does, after its own restarts too.
Upholds=torlek-engine-relay@%i.service

[Install]
WantedBy=multi-user.target
UNIT

mkdir -p "$unit_dir/torlek-engine@$user.service.d"
cat >"$unit_dir/torlek-engine@$user.service.d/torlek.conf" <<UNIT
# Written by Torlek's engine setup for project {project}; running it again rewrites it.
[Service]
# You reach the socket; nobody else does.
ExecStartPre=+$setfacl_program -m u:$owner:x,d:u:$owner:rw /run/$user
{hide_home_engine}UNIT

cat >"$unit_dir/torlek-engine-relay@.service" <<UNIT
# Written by Torlek's engine setup (#390); running a setup again rewrites it.
[Unit]
Description=Torlek's relay to the ports of the rootless Docker engine for %i
BindsTo=torlek-engine@%i.service
After=torlek-engine@%i.service
# The engine's network namespace: its 127.0.0.1 holds only that engine's published ports.
JoinsNamespaceOf=torlek-engine@%i.service

[Service]
User=%i
Group=%i
PrivateNetwork=yes
PrivateTmp=yes
TemporaryFileSystem=/home:ro
InaccessiblePaths={inaccessible}
NoNewPrivileges=yes
Restart=on-failure
RestartSec=2
UNIT

mkdir -p "$unit_dir/torlek-engine-relay@$user.service.d"
cat >"$unit_dir/torlek-engine-relay@$user.service.d/torlek.conf" <<UNIT
# Written by Torlek's engine setup for project {project}; running it again rewrites it.
[Service]
# Your installed program, read-only: whatever can write it runs as $user.
{hide_home_relay}BindReadOnlyPaths=$relay_program
# Serves you alone ({owner_uid}); the socket's folder lets nobody else in either.
ExecStart=$relay_program engine-relay {relay_socket} {owner_uid}
UNIT

# Torlek's masks helper shows this engine the project's checkouts from what it records here.
mkdir -p '{engines_dir}'
user_uid=$(id -u "$user")
cat >"{engines_dir}/$user.conf" <<CONF
# Written by Torlek's engine setup for project {project}; running it again rewrites it.
uid=$user_uid
owner_uid={owner_uid}
project_root=$project_root
clones_dir=$clones_dir
CONF
mkdir -p "$unit_dir/{masks_socket_unit}.d"
cat >"$masks_owner" <<UNIT
# Written by Torlek's engine setup: only you connect, and the helper checks it is you.
[Socket]
SocketUser=$owner
SocketMode=0600
UNIT

systemctl daemon-reload
if systemctl cat {masks_socket_unit} >/dev/null 2>&1; then
    # Its socket becomes yours to connect to.
    systemctl restart {masks_socket_unit}
else
    echo "torlek engine setup: Torlek's masks helper is not installed, so the engine sees no checkout; torlek doctor names its step" >&2
fi
systemctl enable "torlek-engine@$user.service"
# Restarts the relay too: it stops with the engine, which upholds it.
systemctl restart "torlek-engine@$user.service"
echo "torlek engine setup: the engine of {project} runs as $user; now record it: {record}"
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn owner() -> Account {
        Account {
            name: "theo".to_owned(),
            uid: 1000,
            home: PathBuf::from("/home/theo"),
        }
    }

    fn script_for(owner: &Account, root: &str, clones: &str) -> Result<String, String> {
        let user = EngineUser::for_project("torlek", 3).unwrap();
        setup_script(&SetupPlan {
            project: "shop",
            user: &user,
            owner,
            project_root: Path::new(root),
            clones_dir: Path::new(clones),
            record_command: "torlek containers record shop",
            relay_program: Path::new("/home/theo/.local/bin/torlek-sandbox"),
        })
    }

    #[test]
    fn the_engines_unit_shows_it_no_checkout_of_its_own() {
        let script = script_for(
            &owner(),
            "/home/theo/code/shop",
            "/home/theo/code/.parallel/shop",
        )
        .unwrap();

        assert!(!script.contains("BindPaths="), "{script}");
        assert!(script.contains("TemporaryFileSystem=/home\n"), "{script}");
        assert!(!script.contains("TemporaryFileSystem=/home/theo"));
    }

    /// The helper's later mounts reach rootlesskit's namespace below the unit's only so.
    #[test]
    fn the_engines_unit_shares_its_mounts_downstream() {
        let script = script_for(&owner(), "/home/theo/shop", "/home/theo/.parallel/shop").unwrap();

        assert!(script.contains("MountFlags=shared\n"), "{script}");
    }

    #[test]
    fn the_script_keeps_what_containers_make_in_the_checkouts_the_owners() {
        let script = script_for(
            &owner(),
            "/home/theo/code/shop",
            "/home/theo/code/.parallel/shop",
        )
        .unwrap();

        assert!(script.contains("user='torlek-e3'"), "{script}");
        assert!(script.contains("project_root='/home/theo/code/shop'"));
        assert!(script.contains("d:u:$owner:rwX"));
        assert!(script.contains("PrivateNetwork=yes"));
        assert!(script.contains("now record it: torlek containers record shop"));
    }

    #[test]
    fn the_script_records_the_engine_for_the_masks_helper() {
        let script = script_for(&owner(), "/home/theo/shop", "/home/theo/.parallel/shop").unwrap();

        assert!(
            script.contains(
                "cat >\"/etc/torlek/engines/$user.conf\" <<CONF\n\
                 # Written by Torlek's engine setup for project shop; running it again rewrites it.\n\
                 uid=$user_uid\nowner_uid=1000\nproject_root=$project_root\n\
                 clones_dir=$clones_dir\nCONF\n"
            ),
            "{script}"
        );
    }

    /// `set -e` stops at a failed assignment, never at a substitution inside a heredoc.
    #[test]
    fn the_engine_users_uid_is_read_where_a_failure_stops_the_script() {
        let script = script_for(&owner(), "/home/theo/shop", "/home/theo/.parallel/shop").unwrap();

        assert!(
            script.contains("\nuser_uid=$(id -u \"$user\")\n"),
            "{script}"
        );
    }

    /// The helper's socket has one owner per host: a second owner's setup would take it.
    #[test]
    fn a_masks_socket_another_owner_holds_stops_the_script_before_it_changes_anything() {
        let script = script_for(&owner(), "/home/theo/shop", "/home/theo/.parallel/shop").unwrap();

        let refusal = script
            .find("grep -qx \"SocketUser=$owner\" \"$masks_owner\" ||")
            .expect(&script);
        let first_change = script.find("useradd --system").unwrap();
        assert!(refusal < first_change, "{script}");
        assert!(
            script.contains("Torlek's masks helper serves another user on this host"),
            "{script}"
        );
    }

    #[test]
    fn the_masks_socket_lets_the_owner_alone_connect() {
        let script = script_for(&owner(), "/home/theo/shop", "/home/theo/.parallel/shop").unwrap();

        assert!(
            script.contains("[Socket]\nSocketUser=$owner\nSocketMode=0600\n"),
            "{script}"
        );
    }

    #[test]
    fn a_clones_folder_and_checkout_inside_one_another_are_refused() {
        for (root, clones) in [
            ("/home/theo/shop", "/home/theo/shop/.parallel"),
            (
                "/home/theo/.parallel/shop/main",
                "/home/theo/.parallel/shop",
            ),
        ] {
            let error = script_for(&owner(), root, clones).unwrap_err();

            assert!(error.contains("inside one another"), "{root}: {error}");
        }
    }

    #[test]
    fn the_relay_joins_the_engines_network_and_serves_the_owner_alone() {
        let script = script_for(&owner(), "/home/theo/shop", "/home/theo/.parallel/shop").unwrap();

        assert!(
            script.contains("JoinsNamespaceOf=torlek-engine@%i.service\n"),
            "{script}"
        );
        assert!(script.contains("relay_program='/home/theo/.local/bin/torlek-sandbox'"));
        assert!(script.contains("BindReadOnlyPaths=$relay_program\n"));
        assert!(
            script
                .contains("ExecStart=$relay_program engine-relay /run/torlek-e3/relay.sock 1000\n")
        );
        assert!(script.contains("Upholds=torlek-engine-relay@%i.service\n"));
        assert!(script.contains("BindsTo=torlek-engine@%i.service\n"));
        assert!(script.contains("-perm -0001"));
    }

    #[test]
    fn a_relay_program_a_unit_cannot_run_is_refused() {
        let user = EngineUser::for_project("torlek", 3).unwrap();

        let error = setup_script(&SetupPlan {
            project: "shop",
            user: &user,
            owner: &owner(),
            project_root: Path::new("/home/theo/shop"),
            clones_dir: Path::new("/home/theo/.parallel/shop"),
            record_command: "torlek containers record shop",
            relay_program: Path::new("/home/theo/my bin/torlek-sandbox"),
        })
        .unwrap_err();

        assert!(error.contains("the sandbox launcher"), "{error}");
    }

    #[test]
    fn a_relay_program_in_a_folder_the_units_hide_is_refused() {
        let user = EngineUser::for_project("torlek", 3).unwrap();

        let error = setup_script(&SetupPlan {
            project: "shop",
            user: &user,
            owner: &owner(),
            project_root: Path::new("/home/theo/shop"),
            clones_dir: Path::new("/home/theo/.parallel/shop"),
            record_command: "torlek containers record shop",
            relay_program: Path::new("/opt/torlek/bin/torlek-sandbox"),
        })
        .unwrap_err();

        assert!(error.contains("is under /opt"), "{error}");
    }

    #[test]
    fn both_units_hide_the_same_folders() {
        let script = script_for(&owner(), "/home/theo/shop", "/home/theo/.parallel/shop").unwrap();

        assert_eq!(
            script
                .matches("InaccessiblePaths=-/root -/mnt -/media -/srv -/opt -/var/log -/boot\n")
                .count(),
            2,
            "{script}"
        );
    }

    #[test]
    fn a_home_outside_home_is_hidden_too() {
        let owner = Account {
            home: PathBuf::from("/var/home/theo"),
            ..owner()
        };

        let script = script_for(
            &owner,
            "/var/home/theo/shop",
            "/var/home/theo/.parallel/shop",
        )
        .unwrap();

        assert!(
            script.contains("TemporaryFileSystem=/var/home/theo\nUNIT"),
            "{script}"
        );
        assert!(script.contains("TemporaryFileSystem=/var/home/theo:ro\nBindReadOnlyPaths="));
    }

    #[test]
    fn a_checkout_outside_the_owners_home_is_refused() {
        let error = script_for(&owner(), "/srv/shop", "/srv/.parallel/shop").unwrap_err();

        assert!(error.contains("not inside your home"), "{error}");
    }

    #[test]
    fn a_path_a_unit_cannot_bind_is_refused() {
        for root in [
            "/home/theo/my shop",
            "/home/theo/shop:x",
            "/home/theo/100%",
            "/home/theo/it's",
        ] {
            let error = script_for(&owner(), root, "/home/theo/.parallel/shop").unwrap_err();

            assert!(error.contains("cannot take"), "{root}: {error}");
        }
    }

    #[test]
    fn a_path_that_climbs_out_of_the_home_is_refused() {
        let error = script_for(
            &owner(),
            "/home/theo/../../srv/shop",
            "/home/theo/.parallel/shop",
        )
        .unwrap_err();

        assert!(error.contains("holds `..`"), "{error}");
    }

    #[test]
    fn the_engine_home_is_its_own_and_both_id_ranges_match() {
        let script = script_for(&owner(), "/home/theo/shop", "/home/theo/.parallel/shop").unwrap();

        assert!(script.contains("chmod 0700 \"/var/lib/$user\""), "{script}");
        assert!(script.contains("--add-subuids \"$range\""), "{script}");
        assert!(script.contains("--add-subgids \"$range\""), "{script}");
    }

    #[test]
    fn the_script_is_valid_shell() {
        let dir = tempfile::tempdir().unwrap();
        let script = script_for(&owner(), "/home/theo/shop", "/home/theo/.parallel/shop").unwrap();
        let user = EngineUser::for_project("torlek", 3).unwrap();
        let path = write_setup_script(dir.path(), &user, &script).unwrap();

        let checked = std::process::Command::new("sh")
            .arg("-n")
            .arg(&path)
            .output()
            .unwrap();

        assert!(
            checked.status.success(),
            "{}",
            String::from_utf8_lossy(&checked.stderr)
        );
        assert_eq!(path, dir.path().join("torlek-e3-setup.sh"));
    }

    #[test]
    fn the_written_script_is_the_owners_alone() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let user = EngineUser::for_project("torlek", 3).unwrap();

        write_setup_script(dir.path(), &user, "echo first\n").unwrap();
        let path = write_setup_script(dir.path(), &user, "echo second\n").unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "echo second\n");
    }
}
