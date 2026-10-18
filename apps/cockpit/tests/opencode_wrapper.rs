//! The opencode wrapper (config/bin/.local/bin/opencode): Greywall around OpenCode, state kept in
//! the sandbox's private /tmp.

mod common;
use common::sandbox::{bin, write_executable};

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Separates the fakes' records: arguments may hold spaces or be empty.
const RECORD: char = '\u{1e}';

struct Fixture {
    home: PathBuf,
    wrapper: PathBuf,
}

/// What the fake application saw.
struct Launch {
    arguments: Vec<String>,
    environment: HashMap<String, String>,
    greywall_flags: Vec<String>,
}

fn executable(path: &Path, body: &str) {
    write_executable(path, body);
}

fn records(text: &str) -> Vec<String> {
    text.split_terminator(RECORD).map(str::to_string).collect()
}

impl Fixture {
    fn new() -> Fixture {
        let home = std::env::temp_dir().join(format!("opencode-wrapper-{}", cockpit::util::token_hex(6)));
        fs::create_dir_all(home.join(".local/bin")).unwrap();
        // Replace only the sandbox launcher and application, not the wrapper.
        // A real nested Greywall launch requires host namespace privileges.
        executable(
            &home.join(".local/bin/greywall"),
            "#!/usr/bin/env bash\n\
             flags=()\n\
             while [[ $1 != -- ]]; do flags+=(\"$1\"); shift; done\n\
             shift\n\
             printf '%s\\x1e' \"${flags[@]}\" > \"$HOME/greywall-flags\"\n\
             exec \"$@\"\n",
        );
        executable(
            &home.join(".opencode/bin/opencode"),
            "#!/usr/bin/env bash\n\
             printf '%s\\x1e' \"$@\" > \"$HOME/application-arguments\"\n\
             for name in XDG_STATE_HOME HOME XDG_DATA_HOME XDG_CONFIG_HOME XDG_CACHE_HOME; do\n\
             [[ -v $name ]] && printf '%s=%s\\x1e' \"$name\" \"${!name}\"\n\
             done > \"$HOME/application-environment\"\n",
        );
        // Also exercise the legacy wrapper safely: it hard-coded /home/theo.
        // Rebase that home prefix into the fixture, leaving its logic unchanged.
        let wrapper = home.join("wrapper");
        let source = fs::read_to_string(bin().join("opencode")).unwrap();
        fs::write(&wrapper, source.replace("/home/theo/", &format!("{}/", home.display()))).unwrap();
        Fixture { home, wrapper }
    }

    fn run_wrapper(&self, arguments: &[&str]) -> Launch {
        let output = Command::new("bash")
            .arg(&self.wrapper)
            .args(arguments)
            .env("HOME", &self.home)
            .env("XDG_STATE_HOME", self.home.join("host-state"))
            .env("XDG_DATA_HOME", self.home.join("data"))
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .env("XDG_CACHE_HOME", self.home.join("cache"))
            .output()
            .unwrap();
        assert!(output.status.success(), "wrapper failed: {}", String::from_utf8_lossy(&output.stderr));
        let read = |name: &str| fs::read_to_string(self.home.join(name)).unwrap();
        Launch {
            arguments: records(&read("application-arguments")),
            environment: records(&read("application-environment"))
                .into_iter()
                .map(|record| {
                    let (name, value) = record.split_once('=').unwrap();
                    (name.to_string(), value.to_string())
                })
                .collect(),
            greywall_flags: records(&read("greywall-flags")),
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.home);
    }
}

fn flag_value<'a>(flags: &'a [String], flag: &str) -> &'a str {
    let position = flags.iter().position(|candidate| candidate == flag).unwrap_or_else(|| panic!("{flag} missing from {flags:?}"));
    &flags[position + 1]
}

#[test]
fn registration_uses_sandbox_tmp_not_shared_host_state() {
    let fixture = Fixture::new();
    let launch = fixture.run_wrapper(&["debug", "paths", "state"]);
    assert_eq!(launch.environment.get("XDG_STATE_HOME").map(String::as_str), Some("/tmp/opencode-greywall-state"));
    assert!(!fixture.home.join("host-state").exists());
}

#[test]
fn preserves_application_paths_and_arguments() {
    let fixture = Fixture::new();
    let arguments = ["run", "a prompt with spaces", "", "--standalone"];
    let launch = fixture.run_wrapper(&arguments);
    assert_eq!(launch.arguments, arguments);
    assert_eq!(launch.environment["HOME"], fixture.home.display().to_string());
    for (name, directory) in [("XDG_DATA_HOME", "data"), ("XDG_CONFIG_HOME", "config"), ("XDG_CACHE_HOME", "cache")] {
        assert_eq!(launch.environment.get(name), Some(&fixture.home.join(directory).display().to_string()), "{name}");
    }
}

#[test]
fn keeps_opencode_sandbox_profile_and_proxy_identity() {
    let fixture = Fixture::new();
    let flags = fixture.run_wrapper(&["--version"]).greywall_flags;
    assert_eq!(flag_value(&flags, "--profile"), "opencode");
    assert_eq!(flag_value(&flags, "--proxy-user"), "opencode");
    assert_eq!(flag_value(&flags, "--allow-path"), fixture.home.join(".cache/chrome-sandbox").display().to_string());
}
