//! `cockpit daemon install`: the active profile's daemon as a login service (#196): the
//! systemd user unit `cockpit[-<name>].service`, or the launchd agent on macOS, started now.
//! Until the cutover (#66) the default profile belongs to the legacy Cockpit while one is
//! installed, so it is refused then and `--profile next` proposed (#134). Another profile also
//! gets its command, `cockpit-<name>` beside this program, whose name selects the profile.

use std::ffi::OsString;
use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use cockpit_config::Profile;

use crate::daemon::fail;
use crate::installations::{Legacy, ServiceManager, profile_of_unit};

const DAEMON_PROGRAM: &str = "cockpit-daemon";

/// The profile proposed beside a legacy Cockpit (#134).
const COEXISTING_PROFILE: &str = "next";

pub(crate) fn run(arguments: &[OsString]) -> Result<ExitCode, String> {
    if let Some(extra) = arguments.first() {
        return Err(format!(
            "unknown argument {}: select the profile with `cockpit --profile NAME daemon install`",
            extra.to_string_lossy()
        ));
    }
    Ok(install().unwrap_or_else(|problem| fail(&problem)))
}

fn install() -> Result<ExitCode, String> {
    let profile = &crate::profile::active().value;
    let environment = crate::profile::environment();
    let manager = ServiceManager::of_this_platform();
    let unit_dir = manager.unit_dir(&environment)?;
    // Canonical: the daemon and the profile's command sit beside the installed program, not
    // beside a link to it on `PATH`.
    let program = crate::profile::this_program()?;
    let program = program
        .canonicalize()
        .map_err(|error| format!("{}: {error}", program.display()))?;
    if profile.is_default() {
        let legacy = Legacy::find(&unit_dir, std::env::var_os("PATH").as_deref(), &program);
        if legacy.is_found() {
            return Err(format!(
                "a legacy Cockpit is installed ({}): the default profile stays its until the \
                 cutover; install this one beside it with \
                 `cockpit --profile {COEXISTING_PROFILE} daemon install`, which also installs \
                 the command cockpit-{COEXISTING_PROFILE}",
                legacy.describe()
            ));
        }
    }
    let daemon = program.with_file_name(DAEMON_PROGRAM);
    if !daemon.is_file() {
        return Err(format!(
            "{} is missing: install {DAEMON_PROGRAM} beside cockpit",
            daemon.display()
        ));
    }
    let unit = unit_dir.join(manager.unit_file_name(profile));
    refuse_a_foreign_unit(&unit, profile)?;
    if !profile.is_default() {
        link_command(&program, profile)?;
    }
    let variables = unit_variables()?;
    let text = match manager {
        ServiceManager::Systemd => systemd_unit(profile, &daemon, &variables)?,
        ServiceManager::Launchd => launchd_agent(profile, &daemon, &variables)?,
    };
    write_unit(&unit_dir, &unit, &text)?;
    outln!("wrote {}", unit.display());
    match manager {
        ServiceManager::Systemd => {
            let unit_name = profile.systemd_unit();
            service_manager("systemctl", &["--user", "daemon-reload"])?;
            service_manager("systemctl", &["--user", "enable", &unit_name])?;
            // `restart`, not `enable --now`: a reinstall must replace a running daemon.
            service_manager("systemctl", &["--user", "restart", &unit_name])?;
        }
        ServiceManager::Launchd => {
            // The file was just written by this user: its owner is the user's launchd domain.
            let uid = fs::metadata(&unit)
                .map_err(|error| format!("{}: {error}", unit.display()))?
                .uid();
            let domain = format!("gui/{uid}");
            // A previous install of this profile is replaced; none loaded is not a failure.
            let _ = Command::new("launchctl")
                .args(["bootout", &format!("{domain}/{}", profile.launchd_label())])
                .status();
            bootstrap(&domain, &utf8(&unit)?)?;
        }
    }
    outln!(
        "the daemon of profile {profile} runs as {}",
        manager.unit_file_name(profile)
    );
    Ok(ExitCode::SUCCESS)
}

/// `cockpit-<name>` beside this program, pointing at it; an existing link to it is kept, any
/// other file of that name refused.
fn link_command(program: &Path, profile: &Profile) -> Result<(), String> {
    let link = program.with_file_name(profile.qualified_name());
    let target = program
        .canonicalize()
        .map_err(|error| format!("{}: {error}", program.display()))?;
    match fs::symlink_metadata(&link) {
        Ok(_) if link.canonicalize().ok().as_ref() == Some(&target) => return Ok(()),
        Ok(_) => {
            return Err(format!(
                "{} exists and is not this cockpit: remove it or choose another profile",
                link.display()
            ));
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(format!("{}: {error}", link.display())),
    }
    std::os::unix::fs::symlink(&target, &link)
        .map_err(|error| format!("cannot link {}: {error}", link.display()))?;
    outln!("linked {} -> {}", link.display(), target.display());
    Ok(())
}

fn systemd_unit(
    profile: &Profile,
    daemon: &Path,
    variables: &[(&str, String)],
) -> Result<String, String> {
    let daemon = systemd_quoted(&utf8(daemon)?);
    let environment: String = variables
        .iter()
        .map(|(name, value)| {
            format!(
                "Environment={}\n",
                systemd_quoted(&format!("{name}={value}"))
            )
        })
        .collect();
    let name = profile.name();
    let marker = ServiceManager::marker(profile);
    Ok(format!(
        "# Written by `cockpit daemon install`; `cockpit doctor` reads the profile line.\n\
         [Unit]\n\
         Description=Cockpit daemon (profile {name})\n\
         {marker}\n\
         \n\
         [Service]\n\
         Type=simple\n\
         {environment}\
         ExecStart={daemon} --profile {name}\n\
         Restart=on-failure\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n"
    ))
}

/// systemd's double-quoted word: `\` and `"` escaped, `%` doubled so no specifier expands.
fn systemd_quoted(word: &str) -> String {
    let escaped = word
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('%', "%%");
    format!("\"{escaped}\"")
}

fn launchd_agent(
    profile: &Profile,
    daemon: &Path,
    variables: &[(&str, String)],
) -> Result<String, String> {
    let label = xml_escaped(&profile.launchd_label());
    let daemon = xml_escaped(&utf8(daemon)?);
    let environment: String = variables
        .iter()
        .map(|(name, value)| {
            format!(
                "\t\t<key>{name}</key>\n\t\t<string>{}</string>\n",
                xml_escaped(value)
            )
        })
        .collect();
    let name = xml_escaped(profile.name());
    let marker = ServiceManager::marker(profile);
    Ok(format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <!-- Written by `cockpit daemon install`; `cockpit doctor` reads the profile line.\n\
         {marker}\n\
         -->\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \t<key>Label</key>\n\
         \t<string>{label}</string>\n\
         \t<key>ProgramArguments</key>\n\
         \t<array>\n\
         \t\t<string>{daemon}</string>\n\
         \t\t<string>--profile</string>\n\
         \t\t<string>{name}</string>\n\
         \t</array>\n\
         \t<key>EnvironmentVariables</key>\n\
         \t<dict>\n\
         {environment}\
         \t</dict>\n\
         \t<key>RunAtLoad</key>\n\
         \t<true/>\n\
         \t<key>KeepAlive</key>\n\
         \t<dict>\n\
         \t\t<key>SuccessfulExit</key>\n\
         \t\t<false/>\n\
         \t</dict>\n\
         </dict>\n\
         </plist>\n"
    ))
}

fn xml_escaped(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The variables the daemon must see as this command does: `PATH` for the agents it launches,
/// and the XDG bases this command resolved the profile's directories from, so `doctor` and the
/// daemon agree on them. A service manager's own environment has none of the shell's.
fn unit_variables() -> Result<Vec<(&'static str, String)>, String> {
    let mut variables = Vec::new();
    for name in [
        "PATH",
        "XDG_CONFIG_HOME",
        "XDG_STATE_HOME",
        "XDG_CACHE_HOME",
    ] {
        let Some(value) = std::env::var_os(name).filter(|value| !value.is_empty()) else {
            continue;
        };
        let value = value
            .into_string()
            .map_err(|_| format!("{name} is not UTF-8: the unit cannot carry it"))?;
        variables.push((name, value));
    }
    Ok(variables)
}

/// An existing unit of this name is replaced only when `cockpit daemon install` wrote it for
/// this profile: the legacy Cockpit's `cockpit-remote.service` and its kind stay untouched.
fn refuse_a_foreign_unit(unit: &Path, profile: &Profile) -> Result<(), String> {
    if !unit.exists() {
        return Ok(());
    }
    if profile_of_unit(unit)?.as_ref() == Some(profile) {
        return Ok(());
    }
    Err(format!(
        "{} exists and was not written by `cockpit daemon install` for profile {profile}: \
         remove it or choose another profile",
        unit.display()
    ))
}

/// `launchctl bootout` returns before the old job is gone, so the first bootstraps of a
/// reinstall can fail while it unloads; tried a few times before the failure is reported.
fn bootstrap(domain: &str, plist: &str) -> Result<(), String> {
    const ATTEMPTS: u32 = 5;
    for _ in 1..ATTEMPTS {
        if service_manager("launchctl", &["bootstrap", domain, plist]).is_ok() {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    service_manager("launchctl", &["bootstrap", domain, plist])
}

/// Written beside, then renamed over the unit, so a service manager never reads half a file.
fn write_unit(unit_dir: &Path, unit: &Path, text: &str) -> Result<(), String> {
    fs::create_dir_all(unit_dir).map_err(|error| format!("{}: {error}", unit_dir.display()))?;
    let mut partial = unit.as_os_str().to_owned();
    partial.push(".partial");
    let partial = PathBuf::from(partial);
    fs::write(&partial, text).map_err(|error| format!("{}: {error}", partial.display()))?;
    fs::rename(&partial, unit).map_err(|error| format!("{}: {error}", unit.display()))
}

fn service_manager(program: &str, arguments: &[&str]) -> Result<(), String> {
    let shown = format!("{program} {}", arguments.join(" "));
    let status = Command::new(program)
        .args(arguments)
        .status()
        .map_err(|error| format!("cannot run {shown}: {error}"))?;
    if !status.success() {
        return Err(format!("{shown} failed: {status}"));
    }
    Ok(())
}

fn utf8(path: &Path) -> Result<String, String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("{}: not UTF-8", path.display()))
}
