//! `torlek update` and the update notice (#617). A newer release is announced and the owner
//! chooses: update (the exact commands for this installation, printed, never run), later
//! (postpone the notice three days), skip this version, or turn automatic checks off.
//!
//! A check only asks the release list of the official repository for text. It downloads,
//! installs, restarts and migrates nothing, on any path: the owner runs the printed commands
//! themselves, and `torlek daemon install` (the existing way to replace a running daemon)
//! is the last of them. Decided with the owner on 2026-11-04: check at most once a day, only
//! when an interactive command runs, within 3 seconds, cached; see
//! `docs/decisions/2026-11-04-617-update-notice.md`.

mod release;
mod state;

use std::ffi::OsString;
use std::fs;
use std::io::{ErrorKind, IsTerminal};
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};

use crate::daemon::{fail, utf8_words};
use release::{Installed, Latest, Offer};
use state::{POSTPONE_SECONDS, UpdateState};

const REPOSITORY_URL: &str = "https://github.com/Androlax2/agent-cockpit";
const RELEASE_LIST_URL: &str =
    "https://api.github.com/repos/Androlax2/agent-cockpit/releases?per_page=30";
/// A slow or absent network must cost an interactive command no more than this.
const CHECK_TIMEOUT_SECONDS: &str = "3";
const RELEASE_LIST_LIMIT_BYTES: &str = "2097152";
const NOTES_EXCERPT_LINES: usize = 15;
const STATE_FILE: &str = "update.json";

enum Verb {
    Show,
    Check,
    Later,
    Skip,
    Checks { is_on: bool },
}

pub(crate) fn run(arguments: &[OsString]) -> Result<ExitCode, String> {
    let verb = parse(arguments)?;
    Ok(execute(verb).unwrap_or_else(|problem| fail(&problem)))
}

fn parse(arguments: &[OsString]) -> Result<Verb, String> {
    match utf8_words(arguments)?[..] {
        [] => Ok(Verb::Show),
        ["check"] => Ok(Verb::Check),
        ["later"] => Ok(Verb::Later),
        ["skip"] => Ok(Verb::Skip),
        ["checks", "on"] => Ok(Verb::Checks { is_on: true }),
        ["checks", "off"] => Ok(Verb::Checks { is_on: false }),
        ["checks"] => Err("update checks needs on or off".to_owned()),
        ref words => Err(format!("unknown update argument {}", words.join(" "))),
    }
}

/// This installation: what it is, where its files are, and where its update choices live.
struct Installation {
    installed: Installed,
    prefix: PathBuf,
    /// Where the README's archive route unpacks releases: the only place the printed commands
    /// can replace an installation.
    readme_unpack_dir: PathBuf,
    state_path: PathBuf,
}

/// `None` for a build outside a release bundle, which has no release to compare.
fn find_installation() -> Result<Option<Installation>, String> {
    let program = crate::profile::this_program()?;
    let program = program
        .canonicalize()
        .map_err(|error| format!("{}: {error}", program.display()))?;
    let prefix = program
        .parent()
        .and_then(|binary_dir| binary_dir.parent())
        .ok_or_else(|| format!("{} has no prefix", program.display()))?
        .to_path_buf();
    let manifest_path = prefix.join(cockpit_config::MANIFEST_IN_PREFIX);
    let manifest = match fs::read_to_string(&manifest_path) {
        Ok(manifest) => manifest,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{}: {error}", manifest_path.display())),
    };
    let installed = Installed::from_manifest(&manifest)
        .map_err(|problem| format!("{}: {problem}", manifest_path.display()))?;
    let environment = crate::profile::environment();
    let home = environment.home_dir().map_err(|error| error.to_string())?;
    let state_path = cockpit_config::Layout::resolve(&environment, &home)
        .state_dir
        .value
        .join(STATE_FILE);
    let readme_unpack_dir = home.join(".local/lib/torlek");
    Ok(Some(Installation {
        installed,
        prefix,
        readme_unpack_dir,
        state_path,
    }))
}

pub(crate) fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn execute(verb: Verb) -> Result<ExitCode, String> {
    let Some(installation) = find_installation()? else {
        outln!(
            "this torlek is not from a release bundle, so it has no release to compare: \
             nothing to check"
        );
        return Ok(ExitCode::SUCCESS);
    };
    let mut state = state::load(&installation.state_path)?;
    match verb {
        Verb::Show => show(&installation, &state),
        Verb::Check => check(&installation, &mut state),
        Verb::Later => {
            let Some(offer) = state.known_offer(&installation.installed).cloned() else {
                return Ok(fail(
                    "no newer release is known, so there is nothing to postpone",
                ));
            };
            state.postpone(now());
            state::save(&installation.state_path, &state)?;
            outln!(
                "no reminders for {} days; {} stays on offer: `torlek update` shows how",
                POSTPONE_SECONDS / (24 * 60 * 60),
                offer.version
            );
            Ok(ExitCode::SUCCESS)
        }
        Verb::Skip => {
            let Some(offer) = state.known_offer(&installation.installed).cloned() else {
                return Ok(fail(
                    "no newer release is known, so there is nothing to skip",
                ));
            };
            state.skip(offer.version.clone());
            state::save(&installation.state_path, &state)?;
            outln!(
                "skipped {}: no reminders until a newer release is published",
                offer.version
            );
            Ok(ExitCode::SUCCESS)
        }
        Verb::Checks { is_on } => {
            state.automatic_checks = is_on;
            state::save(&installation.state_path, &state)?;
            if is_on {
                outln!("automatic update checks are on: at most one a day");
            } else {
                outln!(
                    "automatic update checks are off; `torlek update check` still asks when you do"
                );
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// `torlek update`: what the last check found and how to update. Reads the cache only, so it
/// answers offline.
fn show(installation: &Installation, state: &UpdateState) -> Result<ExitCode, String> {
    let installed = &installation.installed;
    outln!(
        "installed {} ({}); automatic checks {}",
        installed.version,
        installed.platform,
        if state.automatic_checks { "on" } else { "off" }
    );
    if let Some(skipped) = &state.skipped_version {
        outln!("skipped: {skipped}");
    }
    match state.known_offer(installed) {
        Some(offer) => print_offer(installation, offer),
        None => {
            outln!("no newer release is known; `torlek update check` asks the release list now")
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// `torlek update check`: asks now, whatever the cache, the schedule or the owner's choices
/// about notices say, and records the answer.
fn check(installation: &Installation, state: &mut UpdateState) -> Result<ExitCode, String> {
    let installed = &installation.installed;
    let release_list = fetch_release_list()?;
    let latest = release::latest_release(&release_list, installed)?;
    state.last_attempt_at = Some(now());
    state.latest = match &latest {
        Latest::Available(offer) => Some(offer.clone()),
        Latest::UpToDate | Latest::NoArchiveForPlatform(_) => None,
    };
    state::save(&installation.state_path, state)?;
    match latest {
        Latest::Available(offer) => print_offer(installation, &offer),
        Latest::UpToDate => outln!("torlek {} is the newest release for you", installed.version),
        Latest::NoArchiveForPlatform(version) => outln!(
            "torlek {version} is newer but has no archive for {}, so it is not offered",
            installed.platform
        ),
    }
    Ok(ExitCode::SUCCESS)
}

/// The notice an interactive command prints on stderr when a newer release is due to be
/// announced. Never prints on a pipe, in a launched agent session or with checks off; never
/// fails or blocks beyond the check's timeout, once a day at most.
pub(crate) fn notice_if_due() {
    let is_interactive = std::io::stdout().is_terminal() && std::io::stderr().is_terminal();
    // An agent session's tmux pane is a terminal too; updates are the owner's choice.
    let is_agent_session = std::env::var_os("TORLEK_SESSION").is_some();
    if !is_interactive || is_agent_session {
        return;
    }
    // A failed check must not interrupt the command it rode on: `torlek update` is where a
    // damaged state file or manifest is reported.
    if let Ok(Some(notice)) = due_notice() {
        eprintln!("{notice}");
    }
}

fn due_notice() -> Result<Option<String>, String> {
    let Some(installation) = find_installation()? else {
        return Ok(None);
    };
    let mut state = state::load(&installation.state_path)?;
    let now = now();
    if state.is_check_due(now) {
        // Saved before the request: if the state cannot be written, no request is made, so
        // an unwritable state directory cannot make every command wait for the network.
        state.last_attempt_at = Some(now);
        state::save(&installation.state_path, &state)?;
        let found = fetch_release_list().and_then(|release_list| {
            release::latest_release(&release_list, &installation.installed)
        });
        // A failed check keeps the previous answer; the attempt is recorded all the same.
        if let Ok(latest) = found {
            state.latest = match latest {
                Latest::Available(offer) => Some(offer),
                Latest::UpToDate | Latest::NoArchiveForPlatform(_) => None,
            };
        }
        state::save(&installation.state_path, &state)?;
    }
    let Some(offer) = state
        .offer_to_announce(&installation.installed, now)
        .cloned()
    else {
        return Ok(None);
    };
    state.last_notified_at = Some(now);
    state::save(&installation.state_path, &state)?;
    Ok(Some(announcement(&installation.installed, &offer)))
}

fn announcement(installed: &Installed, offer: &Offer) -> String {
    format!(
        "torlek {} is available (you have {}): {}\n\
         \x20 torlek update          how to update; changes nothing\n\
         \x20 torlek update later    no reminders for 3 days\n\
         \x20 torlek update skip     skip {}\n\
         \x20 torlek update checks off   stop checking",
        offer.version,
        installed.version,
        offer.page_url(),
        offer.version
    )
}

fn print_offer(installation: &Installation, offer: &Offer) {
    let installed = &installation.installed;
    outln!(
        "torlek {} is available (you have {})",
        offer.version,
        installed.version
    );
    let notes: Vec<&str> = offer.notes.lines().collect();
    if !notes.is_empty() {
        outln!("release notes:");
        for line in notes.iter().take(NOTES_EXCERPT_LINES) {
            outln!("  {line}");
        }
    }
    let is_excerpt_cut = notes.len() > NOTES_EXCERPT_LINES;
    outln!(
        "{}: {}",
        if is_excerpt_cut {
            "full release notes"
        } else {
            "release page"
        },
        offer.page_url()
    );
    out!("{}", update_instructions(installation, offer));
    outln!(
        "choices: `torlek update later` (3 days), `torlek update skip`, `torlek update checks off`"
    );
}

/// The concrete update for this installation, as text for the owner to run. Only the archive
/// route of the README exists today (packages are #562, #563 and #565): any other
/// installation is said to be unsupported rather than guessed at.
fn update_instructions(installation: &Installation, offer: &Offer) -> String {
    let installed = &installation.installed;
    let current_archive = installed.archive_name(&installed.version);
    let has_archive_name = installation
        .prefix
        .file_name()
        .is_some_and(|name| *name == *current_archive);
    // The prefix is canonical, so the expected directory is too.
    let unpack_dir = installation
        .readme_unpack_dir
        .canonicalize()
        .unwrap_or_else(|_| installation.readme_unpack_dir.clone());
    let is_unpacked_where_readme_says = installation.prefix.parent() == Some(unpack_dir.as_path());
    if !(has_archive_name && is_unpacked_where_readme_says) {
        return format!(
            "this installation ({}) is not a release archive unpacked in {}, so torlek cannot say how \
             to update it: update it the way you installed it, or install the archive from {}\n",
            installation.prefix.display(),
            installation.readme_unpack_dir.display(),
            REPOSITORY_URL
        );
    }
    let version = &offer.version;
    let tag = offer.tag();
    let archive = installed.archive_name(version);
    let alpha_warning = if installed.channel == release::Channel::Alpha {
        "alpha releases may reset your data: read the release notes first\n"
    } else {
        ""
    };
    format!(
        "to update, run these yourself; nothing has been downloaded or changed:\n\
         \x20 cd \"$(mktemp -d)\"\n\
         \x20 curl -fLO {REPOSITORY_URL}/releases/download/{tag}/{archive}.tar.gz\n\
         \x20 curl -fLO {REPOSITORY_URL}/releases/download/{tag}/SHA256SUMS\n\
         \x20 sha256sum --check --ignore-missing SHA256SUMS &&\n\
         \x20   mkdir -p ~/.local/lib/torlek ~/.local/bin &&\n\
         \x20   tar -xzf {archive}.tar.gz -C ~/.local/lib/torlek &&\n\
         \x20   ln -sfn ~/.local/lib/torlek/{archive}/bin/torlek ~/.local/bin/torlek\n\
         \x20 torlek daemon install\n\
         the last command replaces the running daemon (stop, then start); agent sessions keep running\n\
         {alpha_warning}"
    )
}

/// The release list as text. `curl` is the download tool the install route already needs.
fn fetch_release_list() -> Result<String, String> {
    let output = Command::new("curl")
        .args(["--fail", "--silent", "--show-error", "--location"])
        .args(["--max-time", CHECK_TIMEOUT_SECONDS])
        .args(["--max-filesize", RELEASE_LIST_LIMIT_BYTES])
        .args(["--header", "Accept: application/vnd.github+json"])
        .args(["--header", "X-GitHub-Api-Version: 2022-11-28"])
        .arg(RELEASE_LIST_URL)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("cannot run curl to ask for the release list: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "asking for the release list failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout).map_err(|_| "the release list is not UTF-8".to_owned())
}

#[cfg(test)]
mod tests;
