//! Where notices go: a desktop notification over the session's D-Bus and a short sound, each
//! through a program the desktop already has, so a host without them loses only the notice
//! (#62: a headless run works with notify absent).

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use crate::event::Event;
use crate::notifier::Notice;

/// A program that has not finished by then is stopped: a stuck notification daemon or sound
/// server must not pile up children.
const PROGRAM_DEADLINE: Duration = Duration::from_secs(10);
/// The most notices one desktop notification lists; the rest are counted.
const LISTED_NOTICES: usize = 5;
const APP_NAME: &str = "Torlek";
/// The most characters a session's name or a notice's detail shows: the agent writes them,
/// and a question several paragraphs long must not fill the screen.
const NAME_CHARACTERS: usize = 60;
const DETAIL_CHARACTERS: usize = 200;

/// What the notifier needs to know of the host to find its outlets.
#[derive(Debug, Clone, Default)]
pub struct Host {
    /// `PATH`.
    pub path: Option<OsString>,
    /// Whether a session bus is there to carry a notification: `DBUS_SESSION_BUS_ADDRESS` is
    /// set or `$XDG_RUNTIME_DIR/bus` exists.
    pub has_session_bus: bool,
    /// `XDG_DATA_DIRS`, where the freedesktop sound theme lives.
    pub data_dirs: Vec<PathBuf>,
    /// Whether `DISPLAY` or `WAYLAND_DISPLAY` is set: `canberra-gtk-play` cannot start
    /// without one.
    pub has_display: bool,
}

impl Host {
    pub fn from_env() -> Host {
        let has_bus_address =
            std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some_and(|address| !address.is_empty());
        let has_bus_socket = std::env::var_os("XDG_RUNTIME_DIR")
            .is_some_and(|runtime| Path::new(&runtime).join("bus").exists());
        let data_dirs = match std::env::var_os("XDG_DATA_DIRS").filter(|dirs| !dirs.is_empty()) {
            Some(dirs) => std::env::split_paths(&dirs).collect(),
            None => vec![
                PathBuf::from("/usr/local/share"),
                PathBuf::from("/usr/share"),
            ],
        };
        let has_display = ["DISPLAY", "WAYLAND_DISPLAY"]
            .iter()
            .any(|name| std::env::var_os(name).is_some_and(|display| !display.is_empty()));
        Host {
            path: std::env::var_os("PATH"),
            has_session_bus: has_bus_address || has_bus_socket,
            data_dirs,
            has_display,
        }
    }

    fn program(&self, name: &str) -> Option<PathBuf> {
        let path = self.path.as_ref()?;
        std::env::split_paths(path)
            .map(|dir| dir.join(name))
            .find(|candidate| is_executable(candidate))
    }
}

fn is_executable(path: &Path) -> bool {
    path.metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

/// `notify-send`, which speaks the freedesktop notification protocol over the session bus.
#[derive(Debug, Clone)]
pub struct Desktop {
    program: PathBuf,
}

impl Desktop {
    /// The desktop outlet, or why there is none.
    pub fn find(host: &Host) -> Result<Desktop, String> {
        if !host.has_session_bus {
            return Err("no session bus (DBUS_SESSION_BUS_ADDRESS is unset)".to_owned());
        }
        let program = host
            .program("notify-send")
            .ok_or("notify-send is not on PATH")?;
        Ok(Desktop { program })
    }

    /// One notification for `notices` that landed together: the most urgent first, the rest
    /// listed below it.
    fn arguments(notices: &[&Notice]) -> Vec<String> {
        let mut ordered = notices.to_vec();
        ordered.sort_by_key(|notice| notice.event);
        let Some(first) = ordered.first() else {
            return Vec::new();
        };
        let urgency = if first.event == Event::Failure {
            "critical"
        } else {
            "normal"
        };
        let (summary, body) = if let [only] = ordered.as_slice() {
            let body = only.detail.as_deref().map(body_text).unwrap_or_default();
            (headline(only), body)
        } else {
            let summary = format!("{} ({} more)", headline(first), ordered.len() - 1);
            (summary, listed(&ordered))
        };
        let mut arguments = vec![
            format!("--app-name={APP_NAME}"),
            format!("--urgency={urgency}"),
            "--".to_owned(),
            summary,
        ];
        if !body.is_empty() {
            arguments.push(body);
        }
        arguments
    }

    /// One low-urgency notification of what was held and still needs the owner, once the
    /// last hold ended; `notices` is not empty.
    fn summary_arguments(notices: &[&Notice]) -> Vec<String> {
        let mut ordered = notices.to_vec();
        ordered.sort_by_key(|notice| notice.event);
        let summary = match ordered.len() {
            1 => "While you were held: 1 thing still needs you".to_owned(),
            count => format!("While you were held: {count} things still need you"),
        };
        vec![
            format!("--app-name={APP_NAME}"),
            "--urgency=low".to_owned(),
            "--".to_owned(),
            summary,
            listed(&ordered),
        ]
    }
}

/// `ordered` as a notification body: one line each, up to [`LISTED_NOTICES`], then how many
/// more.
fn listed(ordered: &[&Notice]) -> String {
    let mut lines: Vec<String> = ordered
        .iter()
        .take(LISTED_NOTICES)
        .map(|notice| match &notice.detail {
            Some(detail) => format!("{}: {}", escaped(&headline(notice)), body_text(detail)),
            None => escaped(&headline(notice)),
        })
        .collect();
    let unlisted = ordered.len().saturating_sub(LISTED_NOTICES);
    if unlisted > 0 {
        lines.push(format!("and {unlisted} more"));
    }
    lines.join("\n")
}

fn headline(notice: &Notice) -> String {
    let what = match notice.event {
        Event::Failure => "failed",
        Event::Hold => "waits for your permission",
        Event::Question => "needs your answer",
        Event::Quota => "hit its usage limit",
        Event::Finished => "finished",
        Event::Turn => "completed a turn",
    };
    format!("{} {what}", one_line(&notice.name, NAME_CHARACTERS))
}

/// `detail` as a notification body shows it: one line, cut short, its markup escaped.
fn body_text(detail: &str) -> String {
    escaped(&one_line(detail, DETAIL_CHARACTERS))
}

/// `text` on one line, its runs of white space one space, cut to `limit` characters.
fn one_line(text: &str, limit: usize) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    let line = words.join(" ");
    if line.chars().count() <= limit {
        return line;
    }
    let mut cut: String = line.chars().take(limit - 1).collect();
    cut.push('…');
    cut
}

/// Notification servers read the body as markup (the freedesktop notification
/// specification's `body-markup`): text the agent wrote must show as written.
fn escaped(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// A short sound from the freedesktop sound theme: `canberra-gtk-play` plays it by name from
/// the owner's theme when there is a display; otherwise `pw-play` or `paplay` plays the
/// theme's file.
#[derive(Debug, Clone)]
pub enum Sound {
    Canberra(PathBuf),
    Player {
        program: PathBuf,
        data_dirs: Vec<PathBuf>,
    },
}

impl Sound {
    /// The sound outlet, or why there is none.
    pub fn find(host: &Host) -> Result<Sound, String> {
        if host.has_display
            && let Some(canberra) = host.program("canberra-gtk-play")
        {
            return Ok(Sound::Canberra(canberra));
        }
        let player = ["pw-play", "paplay"]
            .into_iter()
            .find_map(|name| host.program(name))
            .ok_or("no display for canberra-gtk-play, and neither pw-play nor paplay is on PATH")?;
        Ok(Sound::Player {
            program: player,
            data_dirs: host.data_dirs.clone(),
        })
    }

    /// The program and arguments that play `event`'s sound: `file` when there is one, else the
    /// theme's; `None` when the theme's file is missing.
    fn command(&self, event: Event, file: Option<&Path>) -> Option<(PathBuf, Vec<OsString>)> {
        let id = sound_id(event);
        match (self, file) {
            (Sound::Canberra(program), Some(file)) => {
                let mut argument = OsString::from("--file=");
                argument.push(file);
                Some((
                    program.clone(),
                    vec![
                        argument,
                        format!("--description={APP_NAME}: {event}").into(),
                    ],
                ))
            }
            (Sound::Player { program, .. }, Some(file)) => {
                Some((program.clone(), vec![file.as_os_str().to_owned()]))
            }
            (Sound::Canberra(program), None) => Some((
                program.clone(),
                vec![
                    format!("--id={id}").into(),
                    format!("--description={APP_NAME}: {event}").into(),
                ],
            )),
            (Sound::Player { program, data_dirs }, None) => {
                let file = data_dirs
                    .iter()
                    .map(|dir| {
                        dir.join("sounds/freedesktop/stereo")
                            .join(format!("{id}.oga"))
                    })
                    .find(|file| file.is_file())?;
                Some((program.clone(), vec![file.into_os_string()]))
            }
        }
    }
}

/// The theme sound an event with no file of its own plays, by its freedesktop sound-naming
/// name. Every event but `turn` has a bundled file, or the owner's, ahead of it.
pub fn sound_id(event: Event) -> &'static str {
    match event {
        Event::Failure => "dialog-error",
        Event::Hold => "dialog-warning",
        Event::Question => "message-new-instant",
        Event::Quota => "alarm-clock-elapsed",
        Event::Finished => "complete",
        Event::Turn => "message",
    }
}

/// What one outlet did with a notice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outlet {
    Told,
    /// The owner turned the event off for this outlet.
    SettingOff,
    /// The host has no program for this outlet; the start-up log says why.
    NoProgram,
    /// The sound theme has no file for this event; `tell` logs it too.
    NoFile,
    /// A more urgent event that landed with it has the sound.
    Covered,
}

impl Outlet {
    fn label(self) -> &'static str {
        match self {
            Outlet::Told => "told",
            Outlet::SettingOff => "off(setting)",
            Outlet::NoProgram => "off(no program)",
            Outlet::NoFile => "off(no sound file)",
            Outlet::Covered => "off(covered by a more urgent event)",
        }
    }
}

/// The outlets found on this host and the events the owner turned on for each.
#[derive(Debug, Clone, Default)]
pub struct Outlets {
    pub desktop: Option<Desktop>,
    pub desktop_events: BTreeSet<Event>,
    pub sound: Option<Sound>,
    pub sound_events: BTreeSet<Event>,
    /// The file each event's sound plays, the owner's over the bundled one; an event without
    /// one plays its theme sound.
    pub sound_files: BTreeMap<Event, PathBuf>,
}

impl Outlets {
    /// Tells, without a sound, what a hold kept back and still needs the owner (#62): one
    /// low-urgency notification, for the events turned on for either outlet. Returns the
    /// sessions it listed, none when nothing was shown.
    pub fn tell_summary(&self, notices: &[Notice]) -> Vec<String> {
        let Some(desktop) = &self.desktop else {
            return Vec::new();
        };
        // An event on only for sound was kept too: the summary is the one place it is told.
        let shown: Vec<&Notice> = notices
            .iter()
            .filter(|notice| {
                self.desktop_events.contains(&notice.event)
                    || self.sound_events.contains(&notice.event)
            })
            .collect();
        if shown.is_empty() {
            return Vec::new();
        }
        let arguments = Desktop::summary_arguments(&shown);
        start(
            desktop.program.clone(),
            arguments.into_iter().map(Into::into).collect(),
        );
        shown.iter().map(|notice| notice.session.clone()).collect()
    }

    fn sound_command(&self, sound: &Sound, event: Event) -> Option<(PathBuf, Vec<OsString>)> {
        sound.command(event, self.sound_files.get(&event).map(PathBuf::as_path))
    }

    /// Plays `event`'s sound once and waits for the player, whatever the owner's settings say
    /// (`torlek notify test`); returns why it could not.
    pub async fn play(&self, event: Event) -> Result<(), String> {
        let sound = self.sound.as_ref().ok_or("no sound player on this host")?;
        let (program, arguments) = self
            .sound_command(sound, event)
            .ok_or("the sound theme has no file for this event")?;
        let output = tokio::time::timeout(
            PROGRAM_DEADLINE,
            tokio::process::Command::new(&program)
                .args(&arguments)
                .stdin(Stdio::null())
                .kill_on_drop(true)
                .output(),
        )
        .await
        .map_err(|_| format!("{} took too long", program.display()))?
        .map_err(|error| format!("cannot run {}: {error}", program.display()))?;
        if output.status.success() {
            return Ok(());
        }
        Err(format!(
            "{} failed ({}): {}",
            program.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }

    /// Whether the owner turned `event` on for the desktop or the sound.
    pub fn is_wanted(&self, event: Event) -> bool {
        self.desktop_events.contains(&event) || self.sound_events.contains(&event)
    }

    /// The event whose sound plays for `notices` that landed together: the most urgent one
    /// turned on for sound.
    fn loudest_sound(&self, notices: &[Notice]) -> Option<Event> {
        notices
            .iter()
            .map(|notice| notice.event)
            .filter(|event| self.sound_events.contains(event))
            .min()
    }

    /// One line per notice for the log (#1127): whether the desktop and the sound told it
    /// or why not, so a notification the owner finds too many can be traced to its event.
    /// Call it with what [`Outlets::tell`] gets.
    pub fn describe(&self, notices: &[Notice]) -> Vec<String> {
        let loudest = self.loudest_sound(notices);
        notices
            .iter()
            .map(|notice| {
                let desktop = if self.desktop.is_none() {
                    Outlet::NoProgram
                } else if self.desktop_events.contains(&notice.event) {
                    Outlet::Told
                } else {
                    Outlet::SettingOff
                };
                let sound = if self.sound.is_none() {
                    Outlet::NoProgram
                } else if !self.sound_events.contains(&notice.event) {
                    Outlet::SettingOff
                } else if loudest == Some(notice.event) {
                    if self
                        .sound
                        .as_ref()
                        .is_some_and(|sound| self.sound_command(sound, notice.event).is_none())
                    {
                        Outlet::NoFile
                    } else {
                        Outlet::Told
                    }
                } else {
                    Outlet::Covered
                };
                let verb = if desktop == Outlet::Told || sound == Outlet::Told {
                    "told"
                } else {
                    "skipped"
                };
                format!(
                    "{verb} {} session={} name={:?} desktop={} sound={}",
                    notice.event,
                    notice.session,
                    one_line(&notice.name, NAME_CHARACTERS),
                    desktop.label(),
                    sound.label()
                )
            })
            .collect()
    }

    /// Tells the owner about `notices` that landed together: one desktop notification and one
    /// sound, the most urgent event's, each for the events turned on for it. Never waits for
    /// the programs, and a program that fails is only logged.
    pub fn tell(&self, notices: &[Notice]) {
        if let Some(desktop) = &self.desktop {
            let shown: Vec<&Notice> = notices
                .iter()
                .filter(|notice| self.desktop_events.contains(&notice.event))
                .collect();
            if !shown.is_empty() {
                let arguments = Desktop::arguments(&shown);
                start(
                    desktop.program.clone(),
                    arguments.into_iter().map(Into::into).collect(),
                );
            }
        }
        if let Some(sound) = &self.sound {
            let loudest = self.loudest_sound(notices);
            match loudest.map(|event| (event, self.sound_command(sound, event))) {
                Some((_, Some((program, arguments)))) => start(program, arguments),
                Some((event, None)) => {
                    tracing::warn!(%event, "the sound theme has no file for this event");
                }
                None => {}
            }
        }
    }
}

/// Runs `program` in the background, stopped at [`PROGRAM_DEADLINE`].
fn start(program: PathBuf, arguments: Vec<OsString>) {
    let mut command = tokio::process::Command::new(&program);
    command
        .args(&arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    tokio::spawn(async move {
        match tokio::time::timeout(PROGRAM_DEADLINE, command.output()).await {
            Err(_) => tracing::warn!(program = %program.display(), "stopped: it took too long"),
            Ok(Err(error)) => tracing::warn!(program = %program.display(), %error, "cannot run"),
            Ok(Ok(output)) if !output.status.success() => tracing::warn!(
                program = %program.display(),
                status = %output.status,
                stderr = %String::from_utf8_lossy(&output.stderr).trim(),
                "failed"
            ),
            Ok(Ok(_)) => {}
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notice(event: Event, name: &str, detail: Option<&str>) -> Notice {
        Notice {
            event,
            session: "s00000000000a".to_owned(),
            name: name.to_owned(),
            detail: detail.map(str::to_owned),
        }
    }

    #[test]
    fn the_agents_markup_shows_as_written() {
        let asked = notice(Event::Hold, "web", Some("make a && b > <out>"));

        let arguments = Desktop::arguments(&[&asked]);

        assert_eq!(
            arguments.last().unwrap(),
            "make a &amp;&amp; b &gt; &lt;out&gt;"
        );
    }

    #[test]
    fn a_long_question_shows_on_one_shortened_line() {
        let question = format!("Which branch?\n\n{}", "word ".repeat(100));
        let asked = notice(Event::Question, "web", Some(&question));

        let body = Desktop::arguments(&[&asked]).pop().unwrap();

        assert_eq!(body.chars().count(), DETAIL_CHARACTERS);
        assert!(body.starts_with("Which branch? word"), "{body}");
        assert!(body.ends_with('…'), "{body}");
    }

    #[test]
    fn a_long_label_is_shortened_in_the_summary() {
        let finished = notice(Event::Finished, &"x".repeat(100), None);

        let summary = Desktop::arguments(&[&finished]).pop().unwrap();

        assert_eq!(
            summary,
            format!("{}… finished", "x".repeat(NAME_CHARACTERS - 1))
        );
    }

    #[test]
    fn more_than_five_notices_are_counted_past_the_fifth() {
        let notices: Vec<Notice> = (0..7)
            .map(|index| notice(Event::Finished, &format!("s{index}"), None))
            .collect();
        let shown: Vec<&Notice> = notices.iter().collect();

        let body = Desktop::arguments(&shown).pop().unwrap();

        assert_eq!(body.lines().count(), LISTED_NOTICES + 1);
        assert!(body.ends_with("and 2 more"), "{body}");
    }

    fn outlets(desktop_events: &[Event], sound_events: &[Event]) -> Outlets {
        Outlets {
            desktop: Some(Desktop {
                program: PathBuf::from("notify-send"),
            }),
            desktop_events: desktop_events.iter().copied().collect(),
            sound: Some(Sound::Canberra(PathBuf::from("canberra-gtk-play"))),
            sound_events: sound_events.iter().copied().collect(),
            sound_files: BTreeMap::new(),
        }
    }

    #[test]
    fn an_event_with_no_file_plays_its_theme_sound() {
        let sound = Sound::Canberra(PathBuf::from("canberra-gtk-play"));

        let (_, arguments) = sound.command(Event::Turn, None).unwrap();

        assert_eq!(arguments[0], OsString::from("--id=message"));
    }

    #[test]
    fn a_file_replaces_the_theme_sound_for_a_player_too() {
        let sound = Sound::Player {
            program: PathBuf::from("pw-play"),
            data_dirs: Vec::new(),
        };

        let (_, arguments) = sound
            .command(Event::Quota, Some(Path::new("/sounds/quota.oga")))
            .unwrap();

        assert_eq!(arguments, [OsString::from("/sounds/quota.oga")]);
    }

    #[test]
    fn a_completed_turn_is_skipped_until_the_owner_turns_the_event_on() {
        let turn = notice(Event::Turn, "web", None);

        let by_default =
            outlets(&[Event::Finished], &[Event::Finished]).describe(std::slice::from_ref(&turn));
        let turned_on = outlets(&[Event::Turn], &[]).describe(&[turn]);

        assert_eq!(
            by_default,
            [
                r#"skipped turn session=s00000000000a name="web" desktop=off(setting) sound=off(setting)"#
            ]
        );
        assert_eq!(
            turned_on,
            [r#"told turn session=s00000000000a name="web" desktop=told sound=off(setting)"#]
        );
    }

    #[test]
    fn a_sound_a_more_urgent_event_has_is_logged_as_covered() {
        let both = [
            notice(Event::Question, "web", None),
            notice(Event::Finished, "api", None),
        ];

        let lines = outlets(&[], &[Event::Question, Event::Finished]).describe(&both);

        assert!(
            lines[0].ends_with("desktop=off(setting) sound=told"),
            "{lines:?}"
        );
        assert!(
            lines[1].ends_with("sound=off(covered by a more urgent event)"),
            "{lines:?}"
        );
    }

    #[test]
    fn an_outlet_the_host_lacks_is_logged_as_having_no_program() {
        let lines = Outlets::default().describe(&[notice(Event::Failure, "web", None)]);

        assert_eq!(
            lines,
            [
                r#"skipped failure session=s00000000000a name="web" desktop=off(no program) sound=off(no program)"#
            ]
        );
    }
}
