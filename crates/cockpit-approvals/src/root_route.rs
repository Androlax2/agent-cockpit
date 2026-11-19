//! Which `sudo` of a confined run reaches root (#1387, the owner's decision A of 2026-11-19):
//! only a `sudo COMMAND` written in the command line the owner approved. A sudo a script, a
//! makefile, a sourced file or any other program starts is refused, and so is one whose
//! words the line leaves to an expansion or that passes sudo an option: the owner reads every
//! root part on the card.
//!
//! Two checks hold it, each on its own:
//!
//! - **Words.** What the run's `sudo` was given, less `-A`, `-k` and `--`, must be the words of
//!   a `sudo` the line writes out ([`written_sudos`], [`root_argv`]).
//! - **Who started it.** Every process from the `sudo`'s parent up to the run's leader must be
//!   the run's own shell, as launched: the shell's program, with the launch's arguments
//!   ([`started_by_the_run_shell`]). Its subshells and pipeline steps are; a script, `make` or
//!   a program that rewrote its arguments to look like the shell is not. The run's command
//!   ends with `exit $?` ([`root_routed_command`]), so the shell never runs its last step in
//!   its own place.
//!
//! What neither check stops: a file the line sources (`. ./env.sh`) runs in the shell itself,
//! and code of the run that traces the shell (where the host's `ptrace_scope` lets a process
//! trace a non-child) can make it run anything. Either can run a `sudo` the line writes out
//! at another moment, or from another folder; the owner still types the password for it,
//! with its command in the prompt.

use std::ffi::OsString;
use std::path::Path;

use crate::shell::{Simple, lex, skip_assignments, skip_keywords};

/// The most processes walked from a `sudo` up to the run's leader: subshells in subshells.
const ANCESTRY_MAX: usize = 64;

/// The `sudo` commands a command line writes out, as the words their root part runs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct WrittenSudos {
    /// Each root command's words, program first, in the order the line writes them.
    pub(crate) commands: Vec<Vec<String>>,
    /// Each written `sudo` that no forwarded sudo can match, and why.
    pub(crate) unmatched: Vec<String>,
}

/// The `sudo` commands `command` writes out: a simple command whose program, after keywords
/// (`if`, `then`, `!`…) and assignments, is the word `sudo` itself. A `sudo` named by its path,
/// or inside a quoted `sh -c` string, is the run's shell running another program, which the
/// ancestry check refuses anyway.
pub(crate) fn written_sudos(command: &str, home: Option<&Path>) -> WrittenSudos {
    let Some(simples) = lex(command, home) else {
        return WrittenSudos {
            commands: Vec::new(),
            unmatched: vec!["the command line cannot be read, so no sudo in it is".to_owned()],
        };
    };
    let characters: Vec<char> = command.chars().collect();
    let mut written = WrittenSudos::default();
    for simple in &simples {
        let words = skip_assignments(skip_keywords(&simple.words));
        let Some((program, arguments)) = words.split_first() else {
            continue;
        };
        if !program.is_literal || program.text != "sudo" {
            continue;
        }
        let quoted = excerpt(&characters, simple);
        if arguments.iter().any(|word| !word.is_literal) {
            written.unmatched.push(format!(
                "`{quoted}` leaves words to the shell to expand, so it cannot be told from \
                 another sudo"
            ));
            continue;
        }
        let texts: Vec<String> = arguments.iter().map(|word| word.text.clone()).collect();
        match root_argv(&texts) {
            Ok(root) => written.commands.push(root),
            Err(why) => written.unmatched.push(format!("`{quoted}`: {why}")),
        }
    }
    written
}

fn excerpt(characters: &[char], simple: &Simple) -> String {
    let span = simple.span.start..simple.span.end.max(simple.span.start);
    let text: String = characters[span].iter().collect();
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The root command in sudo's `arguments`: what follows `-A`, `-k` (alone or together) and a
/// `--`. Any other option is refused: the root part runs as root, with the floor's limits,
/// and sudo's own choices (`-u`, `-E`, `-s`, `-D`…) would change that unseen.
pub(crate) fn root_argv(arguments: &[String]) -> Result<Vec<String>, String> {
    let mut rest = arguments;
    while let Some((word, after)) = rest.split_first() {
        if word == "--" {
            rest = after;
            break;
        }
        let Some(letters) = word.strip_prefix('-') else {
            break;
        };
        if letters.is_empty() {
            break;
        }
        if !letters.chars().all(|letter| letter == 'A' || letter == 'k') {
            return Err(format!(
                "the root route takes no sudo option but -A and -k, and this names {word}: \
                 write `sudo COMMAND`"
            ));
        }
        rest = after;
    }
    if rest.is_empty() {
        return Err("it names no command to run as root".to_owned());
    }
    Ok(rest.to_vec())
}

/// A file, by its device and inode.
pub(crate) type FileId = (u64, u64);

/// One process, as the ancestry check reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Link {
    pub(crate) parent: u32,
    pub(crate) session: i32,
    pub(crate) cmdline: Vec<OsString>,
    /// The program it runs; `None` when it cannot be read.
    pub(crate) program: Option<FileId>,
}

/// Whether `forwarder`, the run's `sudo`, was started by the run's own shell: it is in the
/// session `leader` leads, and its parent and each process above it up to `leader` runs
/// `shell` with the arguments the run was launched with (`launch`), as a subshell or pipeline
/// step of it does. A process can rewrite its arguments, never its program. `read` reads a
/// process.
pub(crate) fn started_by_the_run_shell(
    forwarder: u32,
    leader: u32,
    launch: &[OsString],
    shell: FileId,
    read: impl Fn(u32) -> Result<Link, String>,
) -> Result<(), String> {
    let session = i32::try_from(leader).map_err(|_| format!("no session is {leader}"))?;
    let sudo = read(forwarder)?;
    if sudo.session != session {
        return Err(format!(
            "process {forwarder} is not in the run's terminal session: nothing was asked"
        ));
    }
    let mut pid = sudo.parent;
    for _ in 0..ANCESTRY_MAX {
        let link = read(pid)?;
        if link.session != session || link.cmdline != launch || link.program != Some(shell) {
            return Err(format!(
                "this sudo was started by process {pid}, not by the command line you approved: \
                 a sudo a script, makefile or other program starts never reaches root"
            ));
        }
        if pid == leader {
            return Ok(());
        }
        pid = link.parent;
    }
    Err(format!(
        "this sudo is more than {ANCESTRY_MAX} subshells below the run's: nothing was asked"
    ))
}

/// The command a root-routed run's shell runs: `command`, then `exit $?`, so its last step is
/// started as a child of the shell, never run in the shell's own place, which would leave a
/// program the line runs (`./x.sh`) where the run's shell was. `None` when `command` ends with
/// a backslash, which would join the two.
pub(crate) fn root_routed_command(command: &str) -> Option<String> {
    let trailing = command
        .trim_end_matches(['\n', ' ', '\t'])
        .chars()
        .rev()
        .take_while(|character| *character == '\\')
        .count();
    (trailing % 2 == 0).then(|| format!("{command}\nexit $?"))
}

/// A process, read from `/proc`.
#[cfg(target_os = "linux")]
pub(crate) fn read_link(pid: u32) -> Result<Link, String> {
    use std::os::unix::ffi::OsStringExt;
    use std::os::unix::fs::MetadataExt;

    let process = crate::sudo::read_process(pid)?;
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline"))
        .map_err(|error| format!("process {pid} cannot be read ({error})"))?;
    let program = std::fs::metadata(format!("/proc/{pid}/exe"))
        .ok()
        .map(|metadata| (metadata.dev(), metadata.ino()));
    Ok(Link {
        parent: process.parent,
        session: process.session,
        program,
        cmdline: cmdline
            .split(|byte| *byte == 0)
            .filter(|word| !word.is_empty())
            .map(|word| OsString::from_vec(word.to_vec()))
            .collect(),
    })
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn read_link(pid: u32) -> Result<Link, String> {
    Err(format!(
        "process {pid} cannot be checked: the root route is Linux's alone"
    ))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn words(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn each_written_sudo_is_its_root_command() {
        let written = written_sudos(
            "make && sudo make install; if true; then sudo -A -k -- cp a /usr/local/bin; fi",
            None,
        );

        assert_eq!(
            written.commands,
            [words("make install"), words("cp a /usr/local/bin")]
        );
        assert!(written.unmatched.is_empty(), "{:?}", written.unmatched);
    }

    #[test]
    fn subshells_and_pipeline_steps_write_their_sudo_too() {
        let written = written_sudos("(cd x && sudo ./install.sh) | sudo tee /etc/x", None);

        assert_eq!(
            written.commands,
            [words("./install.sh"), words("tee /etc/x")]
        );
    }

    #[test]
    fn a_sudo_in_a_shell_string_or_by_its_path_is_not_written() {
        let written = written_sudos("sh -c 'sudo true'; /usr/bin/sudo true; ./setup.sh", None);

        assert_eq!(written, WrittenSudos::default());
    }

    #[test]
    fn an_expanded_or_optioned_sudo_is_named_unmatched() {
        for (line, named) in [
            ("sudo cp $FILE /etc", "expand"),
            ("sudo -u web true", "-u"),
            ("sudo -E make install", "-E"),
            ("sudo -s", "-s"),
            ("sudo -k", "no command"),
        ] {
            let written = written_sudos(line, None);

            assert!(written.commands.is_empty(), "{line}");
            assert!(
                written.unmatched.len() == 1 && written.unmatched[0].contains(named),
                "{line}: {:?}",
                written.unmatched
            );
        }
    }

    #[test]
    fn a_line_that_does_not_lex_writes_no_sudo() {
        let written = written_sudos("sudo echo 'unterminated", None);

        assert!(written.commands.is_empty());
        assert_eq!(written.unmatched.len(), 1);
    }

    #[test]
    fn only_asking_flags_come_off_before_the_root_command() {
        for (given, root) in [
            ("make install", "make install"),
            ("-A -k make install", "make install"),
            ("-Ak -- -weird-program x", "-weird-program x"),
            ("-kA -A make", "make"),
        ] {
            assert_eq!(root_argv(&words(given)), Ok(words(root)), "{given}");
        }
        for given in ["-u root make", "-n make", "--preserve-env make", "-A", ""] {
            assert!(root_argv(&words(given)).is_err(), "{given}");
        }
    }

    const LEADER: u32 = 100;
    const SHELL: FileId = (1, 10);
    const OTHER_PROGRAM: FileId = (1, 20);

    fn launch() -> Vec<OsString> {
        ["/bin/sh", "-c", "sudo make install\nexit $?"]
            .map(OsString::from)
            .to_vec()
    }

    /// `processes`: pid, parent, session, cmdline (empty: the launch's, run by the shell).
    fn table(processes: &[(u32, u32, i32, &[&str])]) -> HashMap<u32, Link> {
        processes
            .iter()
            .map(|(pid, parent, session, cmdline)| {
                let (cmdline, program) = if cmdline.is_empty() {
                    (launch(), SHELL)
                } else {
                    (cmdline.iter().map(OsString::from).collect(), OTHER_PROGRAM)
                };
                (
                    *pid,
                    Link {
                        parent: *parent,
                        session: *session,
                        cmdline,
                        program: Some(program),
                    },
                )
            })
            .collect()
    }

    fn check(processes: &[(u32, u32, i32, &[&str])], forwarder: u32) -> Result<(), String> {
        check_table(&table(processes), forwarder)
    }

    fn check_table(table: &HashMap<u32, Link>, forwarder: u32) -> Result<(), String> {
        started_by_the_run_shell(forwarder, LEADER, &launch(), SHELL, |pid| {
            table
                .get(&pid)
                .cloned()
                .ok_or_else(|| format!("process {pid} is gone"))
        })
    }

    #[test]
    fn a_sudo_the_run_shell_started_is_written_in_the_line() {
        let processes: &[(u32, u32, i32, &[&str])] = &[
            (LEADER, 1, 100, &[]),
            (200, LEADER, 100, &["torlek-sandbox"]),
        ];

        assert_eq!(check(processes, 200), Ok(()));
    }

    #[test]
    fn a_sudo_a_subshell_of_the_run_shell_started_is_written_in_the_line() {
        let processes: &[(u32, u32, i32, &[&str])] = &[
            (LEADER, 1, 100, &[]),
            (150, LEADER, 100, &[]),
            (200, 150, 100, &["torlek-sandbox"]),
        ];

        assert_eq!(check(processes, 200), Ok(()));
    }

    #[test]
    fn a_sudo_a_script_or_make_started_is_refused() {
        for program in [&["/bin/sh", "./setup.sh"][..], &["make", "install"]] {
            let processes: &[(u32, u32, i32, &[&str])] = &[
                (LEADER, 1, 100, &[]),
                (150, LEADER, 100, program),
                (200, 150, 100, &["torlek-sandbox"]),
            ];

            let refused = check(processes, 200).unwrap_err();
            assert!(refused.contains("process 150"), "{refused}");
        }
    }

    #[test]
    fn a_program_that_looks_like_the_run_shell_is_refused() {
        let mut table = table(&[
            (LEADER, 1, 100, &[]),
            (150, LEADER, 100, &[]),
            (200, 150, 100, &["torlek-sandbox"]),
        ]);
        table.get_mut(&150).unwrap().program = Some(OTHER_PROGRAM);

        let refused = check_table(&table, 200).unwrap_err();
        assert!(refused.contains("process 150"), "{refused}");
    }

    #[test]
    fn a_sudo_outside_the_runs_session_is_refused() {
        let processes: &[(u32, u32, i32, &[&str])] = &[
            (LEADER, 1, 100, &[]),
            (200, LEADER, 300, &["torlek-sandbox"]),
        ];

        assert!(check(processes, 200).is_err());
    }

    #[test]
    fn a_sudo_whose_shell_ran_it_in_its_own_place_is_refused() {
        // The shell exec'd it: its parent is the service manager, not the run's shell.
        let processes: &[(u32, u32, i32, &[&str])] = &[
            (LEADER, 1, 100, &["torlek-sandbox"]),
            (1, 0, 1, &["systemd"]),
        ];

        assert!(check(processes, LEADER).is_err());
    }

    #[test]
    fn the_command_ends_with_its_exit_status() {
        assert_eq!(
            root_routed_command("sudo make install").as_deref(),
            Some("sudo make install\nexit $?")
        );
        assert_eq!(
            root_routed_command("echo a \\\\").as_deref(),
            Some("echo a \\\\\nexit $?")
        );
        assert_eq!(root_routed_command("sudo make \\\n"), None);
    }
}
