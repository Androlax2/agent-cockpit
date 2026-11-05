//! The toolchain table (#859): what it declares, how a checkout's ecosystems are detected and
//! which install roots a launch's `PATH` binds.

use std::fs;
use std::path::{Path, PathBuf};

use cockpit_provider_api::toolchains::{COMMON_VARIABLES, Setting, TOOLCHAINS, Toolchain, detect};
use cockpit_provider_api::{HomePath, Host};

fn toolchain(ecosystem: &str) -> &'static Toolchain {
    TOOLCHAINS
        .iter()
        .find(|toolchain| toolchain.ecosystem == ecosystem)
        .unwrap()
}

#[test]
fn every_registry_host_is_a_valid_host() {
    for toolchain in &TOOLCHAINS {
        for host in toolchain.registry_hosts {
            assert!(Host::new(*host).is_ok(), "{host}");
        }
    }
}

#[test]
fn every_install_root_is_a_home_path_with_a_wildcard_only_where_path_names_the_folder() {
    for toolchain in &TOOLCHAINS {
        for group in toolchain.install_roots {
            assert!(
                !group.is_empty(),
                "{} has an empty group",
                toolchain.ecosystem
            );
            for (index, root) in group.iter().enumerate() {
                assert!(HomePath::new(*root).is_ok(), "{root}");
                assert!(index == 0 || !root.contains('*'), "{root}");
            }
        }
    }
}

/// Tool configuration and registry tokens stay outside the sandbox (#859 Q3, #832).
#[test]
fn no_install_root_covers_a_credential_or_a_tool_configuration() {
    let owner_files = [
        "~/.cargo/credentials.toml",
        "~/.cargo/config.toml",
        "~/.npmrc",
        "~/.pypirc",
        "~/.config/pip/pip.conf",
        "~/.config/gh/hosts.yml",
        "~/.netrc",
        "~/.local/bin/tool",
        "~/go/pkg/mod/cache",
    ];
    for toolchain in &TOOLCHAINS {
        for root in toolchain
            .install_roots
            .iter()
            .flat_map(|group| group.iter())
        {
            let root = HomePath::new(*root).unwrap();
            for file in owner_files {
                assert!(
                    !root.covers(&HomePath::new(file).unwrap()),
                    "{root} covers {file}"
                );
            }
        }
    }
}

#[test]
fn every_variable_is_a_name_and_every_session_folder_a_plain_name() {
    let variables = TOOLCHAINS
        .iter()
        .flat_map(|toolchain| toolchain.variables.iter())
        .chain(COMMON_VARIABLES.iter());
    for (name, setting) in variables {
        assert!(
            !name.is_empty()
                && !name.starts_with(|first: char| first.is_ascii_digit())
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'),
            "{name}"
        );
        if let Setting::CacheFolder(folder) | Setting::ConfigFile(folder) = setting {
            assert!(
                !folder.is_empty() && !folder.contains('/') && !folder.starts_with('.'),
                "{name}: {folder}"
            );
        }
    }
}

#[test]
fn the_rust_row_turns_the_owner_s_compiler_wrappers_off() {
    let variables = toolchain("rust").variables;

    for wrapper in [
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "CARGO_BUILD_RUSTC_WRAPPER",
    ] {
        assert!(
            variables.contains(&(wrapper, Setting::Value(""))),
            "{wrapper}"
        );
    }
}

fn ecosystems(checkout: &Path) -> Vec<&'static str> {
    detect(checkout)
        .unwrap()
        .into_iter()
        .map(|toolchain| toolchain.ecosystem)
        .collect()
}

#[test]
fn a_checkout_s_marker_files_name_its_ecosystems_in_table_order() {
    let checkout = tempfile::tempdir().unwrap();
    for marker in ["go.sum", "Cargo.lock", "package.json"] {
        fs::write(checkout.path().join(marker), "").unwrap();
    }

    assert_eq!(ecosystems(checkout.path()), ["rust", "node", "go"]);
}

#[test]
fn a_checkout_without_marker_files_uses_no_ecosystem() {
    let checkout = tempfile::tempdir().unwrap();
    fs::create_dir(checkout.path().join("Cargo.toml")).unwrap();
    fs::write(checkout.path().join("README.md"), "").unwrap();

    assert!(ecosystems(checkout.path()).is_empty());
}

#[test]
fn a_marker_file_that_cannot_be_read_fails_the_detection() {
    let refused = detect(Path::new("/dev/null/checkout")).unwrap_err();

    assert!(
        refused
            .to_string()
            .contains("/dev/null/checkout/Cargo.toml")
    );
}

const HOME: &str = "/home/owner";

fn roots(ecosystem: &str, path: &[&str]) -> Vec<PathBuf> {
    let entries: Vec<PathBuf> = path.iter().map(PathBuf::from).collect();
    toolchain(ecosystem)
        .roots_to_bind(Path::new(HOME), &entries)
        .unwrap()
}

#[test]
fn rustup_s_proxies_on_path_bind_them_and_the_toolchains_they_run() {
    assert_eq!(
        roots("rust", &["/usr/bin", "/home/owner/.cargo/bin"]),
        [
            PathBuf::from("/home/owner/.cargo/bin"),
            PathBuf::from("/home/owner/.rustup")
        ]
    );
}

#[test]
fn a_wildcard_root_binds_only_the_version_path_names() {
    assert_eq!(
        roots(
            "node",
            &["/home/owner/.nvm/versions/node/v22.11.0/bin", "/usr/bin"]
        ),
        [PathBuf::from("/home/owner/.nvm/versions/node/v22.11.0")]
    );
}

#[test]
fn a_path_entry_inside_a_root_binds_the_whole_root() {
    assert_eq!(
        roots("python", &["/home/owner/.pyenv/shims"]),
        [PathBuf::from("/home/owner/.pyenv")]
    );
}

#[test]
fn a_malformed_root_in_the_table_is_refused_not_bound() {
    let malformed = Toolchain {
        install_roots: &[&["/opt/tool/bin"]],
        ..*toolchain("go")
    };

    let refused = malformed
        .roots_to_bind(Path::new(HOME), &[PathBuf::from("/opt/tool/bin")])
        .unwrap_err();

    assert_eq!(refused.value, "/opt/tool/bin");
}

#[test]
fn a_companion_root_with_a_wildcard_is_refused() {
    let malformed = Toolchain {
        install_roots: &[&["~/.cargo/bin", "~/.rustup/toolchains/stable*"]],
        ..*toolchain("rust")
    };

    let refused = malformed
        .roots_to_bind(Path::new(HOME), &[PathBuf::from("/home/owner/.cargo/bin")])
        .unwrap_err();

    assert_eq!(refused.kind, "install root");
}

#[test]
fn a_path_entry_outside_every_root_binds_nothing() {
    for entry in [
        "/usr/bin",
        "/home/owner/.local/bin",
        "/home/owner/.cargo",
        "/home/owner/.cargo/binaries",
        "/home/owner/.nvm/versions/node",
        "/home/owner/.nvm/alias",
    ] {
        for toolchain in &TOOLCHAINS {
            assert!(
                toolchain
                    .roots_to_bind(Path::new(HOME), &[PathBuf::from(entry)])
                    .unwrap()
                    .is_empty(),
                "{entry} binds a root of {}",
                toolchain.ecosystem
            );
        }
    }
}
