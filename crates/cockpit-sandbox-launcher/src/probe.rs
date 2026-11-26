//! The probe the daemon runs at its start: can this host start a sandbox at all, and which
//! Landlock ABI does a sandboxed process get? Prints one JSON line the daemon reads:
//! `{"bubblewrap":"0.13.1","landlock_abi":9}`. A failure is the reason required launches
//! are refused.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

use crate::stage2::PROBE_VERB;

#[derive(Debug, Serialize)]
struct Report {
    bubblewrap: String,
    landlock_abi: u8,
}

pub fn run(bubblewrap: PathBuf) -> Result<(), String> {
    let version = bubblewrap_version(&bubblewrap)?;
    let launcher = std::env::current_exe()
        .and_then(|path| path.canonicalize())
        .map_err(|error| format!("cannot find the launcher's own program: {error}"))?;
    let output = Command::new(&bubblewrap)
        .args(["--unshare-all", "--die-with-parent", "--ro-bind", "/", "/"])
        .args(["--proc", "/proc", "--dev", "/dev", "--"])
        .arg(&launcher)
        .arg(PROBE_VERB)
        .output()
        .map_err(|error| {
            format!(
                "cannot start bubblewrap at {}: {error}",
                bubblewrap.display()
            )
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let bubblewrap_refused = stderr
            .lines()
            .any(|line| line.starts_with(BUBBLEWRAP_PREFIX));
        let diagnosis = if bubblewrap_refused {
            format!(
                "; {}",
                userns_refusal(Path::new(PROC_SYS), &bubblewrap)
                    .unwrap_or_else(|| NO_KERNEL_SETTING_REFUSES.to_owned())
            )
        } else {
            String::new()
        };
        return Err(format!(
            "the probe launch failed ({}): {}{diagnosis}",
            output.status,
            stderr.trim()
        ));
    }
    let landlock_abi = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .map_err(|_| {
            format!(
                "the probe launch printed no Landlock ABI: {:?}",
                String::from_utf8_lossy(&output.stdout)
            )
        })?;
    let report = Report {
        bubblewrap: version,
        landlock_abi,
    };
    let line = serde_json::to_string(&report)
        .map_err(|error| format!("cannot write the probe report: {error}"))?;
    println!("{line}");
    Ok(())
}

const PROC_SYS: &str = "/proc/sys";

/// How bubblewrap's own errors start: it failed to set up the namespaces or mounts before
/// it ran the launcher's stage 2, whose own errors (Landlock, seccomp) name their fix.
const BUBBLEWRAP_PREFIX: &str = "bwrap: ";

/// The fix when no kernel setting explains bubblewrap's failure: most often, Torlek runs in
/// a container that refuses nested namespaces, as a VPS's LXC or OpenVZ guest does.
const NO_KERNEL_SETTING_REFUSES: &str = "the sandbox needs unprivileged user namespaces, and \
    no kernel setting under /proc/sys refuses them: if Torlek runs in a container (Docker, LXC, \
    an OpenVZ or LXC VPS), that container refuses nested namespaces; run Torlek on the host or \
    in a full virtual machine (KVM, a cloud VM), whose kernel gives them";

/// Why the kernel refuses bubblewrap an unprivileged user namespace, read from `proc_sys`,
/// with the command that allows it; `None` when no setting there refuses it.
fn userns_refusal(proc_sys: &Path, bubblewrap: &Path) -> Option<String> {
    let setting = |name: &str| {
        std::fs::read_to_string(proc_sys.join(name))
            .ok()
            .map(|value| value.trim().to_owned())
    };
    if setting("user/max_user_namespaces").as_deref() == Some("0") {
        return Some(
            "the kernel allows no user namespaces (user.max_user_namespaces = 0): run `sudo \
             sysctl -w user.max_user_namespaces=15000`, and write that setting in a file \
             under /etc/sysctl.d to keep it"
                .to_owned(),
        );
    }
    if setting("kernel/unprivileged_userns_clone").as_deref() == Some("0") {
        return Some(
            "the kernel refuses unprivileged user namespaces \
             (kernel.unprivileged_userns_clone = 0): run `sudo sysctl -w \
             kernel.unprivileged_userns_clone=1`, and write that setting in a file under \
             /etc/sysctl.d to keep it"
                .to_owned(),
        );
    }
    if setting("kernel/apparmor_restrict_unprivileged_userns").as_deref() == Some("1") {
        return Some(format!(
            "AppArmor restricts unprivileged user namespaces \
             (kernel.apparmor_restrict_unprivileged_userns = 1, Ubuntu 23.10 and later): unless \
             bubblewrap already has a profile that allows them, write /etc/apparmor.d/bwrap \
             with three lines, `abi <abi/4.0>,`, `include <tunables/global>` and `profile \
             bwrap {} flags=(unconfined) {{ userns, }}`, then run `sudo systemctl reload \
             apparmor`",
            bubblewrap.display()
        ));
    }
    None
}

/// `bubblewrap 0.13.1` gives `0.13.1`.
fn bubblewrap_version(bubblewrap: &Path) -> Result<String, String> {
    let output = Command::new(bubblewrap)
        .arg("--version")
        .output()
        .map_err(|error| {
            format!(
                "cannot start bubblewrap at {}: {error}",
                bubblewrap.display()
            )
        })?;
    let text = String::from_utf8_lossy(&output.stdout);
    match text.trim().split_once(' ') {
        Some(("bubblewrap", version)) if output.status.success() => Ok(version.to_owned()),
        _ => Err(format!(
            "{} --version printed {:?}, not bubblewrap's version",
            bubblewrap.display(),
            text.trim()
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::userns_refusal;

    fn proc_sys(settings: &[(&str, &str)]) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        for (name, value) in settings {
            let file = root.path().join(name);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, format!("{value}\n")).unwrap();
        }
        root
    }

    #[test]
    fn each_kernel_setting_that_refuses_user_namespaces_names_its_own_fix() {
        let cases = [
            (
                "user/max_user_namespaces",
                "0",
                "sysctl -w user.max_user_namespaces=15000",
            ),
            (
                "kernel/unprivileged_userns_clone",
                "0",
                "sysctl -w kernel.unprivileged_userns_clone=1",
            ),
            (
                "kernel/apparmor_restrict_unprivileged_userns",
                "1",
                "profile bwrap /usr/bin/bwrap flags=(unconfined) { userns, }",
            ),
        ];
        for (name, value, fix) in cases {
            let root = proc_sys(&[(name, value)]);

            let refusal = userns_refusal(root.path(), Path::new("/usr/bin/bwrap"));

            let refusal = refusal.unwrap_or_else(|| panic!("{name} = {value} refuses nothing"));
            assert!(refusal.contains(fix), "{refusal}");
        }
    }

    #[test]
    fn settings_that_allow_user_namespaces_refuse_nothing() {
        let root = proc_sys(&[
            ("user/max_user_namespaces", "63416"),
            ("kernel/unprivileged_userns_clone", "1"),
            ("kernel/apparmor_restrict_unprivileged_userns", "0"),
        ]);

        let refusal = userns_refusal(root.path(), Path::new("/usr/bin/bwrap"));

        assert_eq!(refusal, None);
    }

    #[test]
    fn a_kernel_without_the_settings_refuses_nothing() {
        let root = proc_sys(&[]);

        let refusal = userns_refusal(root.path(), Path::new("/usr/bin/bwrap"));

        assert_eq!(refusal, None);
    }
}
