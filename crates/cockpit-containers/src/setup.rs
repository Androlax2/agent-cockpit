//! The guided root step that sets up a project's engine (#390, #856 Q2): a script `torlek
//! doctor` writes for the owner to review and run once with `sudo`. Torlek never runs it.
//!
//! It makes the engine's system user and its subordinate ids, lets that user write the
//! project's checkouts with ACLs whose default entries keep what containers create there the
//! owner's to change and delete, installs the `torlek-engine@` unit with an instance drop-in
//! that binds the checkouts into an otherwise empty `/home`, and restarts the engine. The
//! unit has no network until the engine's egress proxy exists: pulls fail closed.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::{Account, EngineUser};

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
    // Under /home, the unit's empty /home hides it already.
    let hide_home = if home.starts_with("/home") {
        String::new()
    } else {
        format!("TemporaryFileSystem={home_text}:ro\n")
    };
    Ok(SCRIPT
        .replace("{project}", plan.project)
        .replace("{user}", plan.user.name())
        .replace("{owner}", &plan.owner.name)
        .replace("{project_root}", &root)
        .replace("{clones_dir}", &clones)
        .replace("{hide_home}", &hide_home)
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

/// `path` as a unit file and the script take it: absolute UTF-8 without `..` (which would
/// escape the home check), whitespace, quotes, backslashes, `$`, `%` (a unit specifier)
/// or `:` (`BindPaths=`' separator).
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
             engine's unit cannot bind"
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
# write the project's checkouts, installs the unit torlek-engine@{user}.service and starts
# it. Running it again is safe: it rewrites the unit and restarts the engine.
set -eu

user='{user}'
owner='{owner}'
project_root='{project_root}'
clones_dir='{clones_dir}'
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
dockerd_rootless=$(command -v dockerd-rootless.sh)
setfacl_program=$(command -v setfacl)
nologin=$(command -v nologin || echo /bin/false)

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
# No home but the project's checkouts, which the instance's drop-in binds.
TemporaryFileSystem=/home:ro
InaccessiblePaths=-/root -/mnt -/media -/srv -/opt -/var/log -/boot
Delegate=yes
KillMode=mixed
Restart=on-failure
RestartSec=2

[Install]
WantedBy=multi-user.target
UNIT

mkdir -p "$unit_dir/torlek-engine@$user.service.d"
cat >"$unit_dir/torlek-engine@$user.service.d/torlek.conf" <<UNIT
# Written by Torlek's engine setup for project {project}; running it again rewrites it.
[Service]
# You reach the socket; nobody else does.
ExecStartPre=+$setfacl_program -m u:$owner:x,d:u:$owner:rw /run/$user
{hide_home}BindPaths=$project_root
BindPaths=$clones_dir
UNIT

systemctl daemon-reload
systemctl enable "torlek-engine@$user.service"
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
        })
    }

    #[test]
    fn the_script_binds_the_checkouts_and_keeps_containers_files_the_owners() {
        let script = script_for(
            &owner(),
            "/home/theo/code/shop",
            "/home/theo/code/.parallel/shop",
        )
        .unwrap();

        assert!(script.contains("user='torlek-e3'"), "{script}");
        assert!(script.contains("project_root='/home/theo/code/shop'"));
        assert!(script.contains("BindPaths=$project_root\nBindPaths=$clones_dir"));
        assert!(script.contains("d:u:$owner:rwX"));
        assert!(script.contains("PrivateNetwork=yes"));
        assert!(script.contains("now record it: torlek containers record shop"));
        assert!(!script.contains("TemporaryFileSystem=/home/theo"));
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

        assert!(script.contains("TemporaryFileSystem=/var/home/theo:ro\nBindPaths="));
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

            assert!(error.contains("cannot bind"), "{root}: {error}");
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
