//! The toolchain table (#859): for each ecosystem a project may use, the files that say it
//! does, the install roots a session reads from the owner's home, the variables that keep its
//! caches and configuration in the session's own folders, and the registry hosts it fetches
//! from. Seeded from Greywall's `internal/profiles/toolchains` at `a9ccc58c`, Apache-2.0:
//! see `NOTICE-greywall`. The table holds no write access to the owner's home and no
//! credential: tool configuration and registry tokens stay outside (#859 Q3, #832).

use std::io;
use std::path::{Path, PathBuf};

use crate::InvalidValue;
use crate::needs::{HomePath, matches_name};

/// One ecosystem's row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Toolchain {
    pub ecosystem: &'static str,
    /// Files at the checkout's root whose presence says the project uses the ecosystem.
    pub markers: &'static [&'static str],
    /// Groups of install roots in the home (`~/` paths), each bound read-only when the
    /// launch's `PATH` holds a folder inside its first root, which is where that install's
    /// programs resolve; the other roots of the group are what those programs need beside
    /// it. A `*` in the first root's last name binds only the folder `PATH` names there,
    /// such as one Node version of several.
    pub install_roots: &'static [&'static [&'static str]],
    /// The variables that keep the ecosystem's caches and configuration in the session's
    /// own folders.
    pub variables: &'static [(&'static str, Setting)],
    /// The registry hosts a session with an empty cache fetches dependencies from (#831).
    pub registry_hosts: &'static [&'static str],
}

/// The value a variable gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    /// A folder of this name in the session's cache folder.
    CacheFolder(&'static str),
    /// A file of this name Torlek writes in the session's folder, holding no credential.
    ConfigFile(&'static str),
    /// This value as it is; an empty one turns a wrapper off.
    Value(&'static str),
    /// This flag added, after a space, to the value the launch already gives the variable,
    /// so the owner's own flags stay.
    Flag(&'static str),
}

/// Variables every session gets, whatever its ecosystems.
pub const COMMON_VARIABLES: [(&str, Setting); 2] = [
    ("XDG_CACHE_HOME", Setting::CacheFolder("xdg")),
    ("CCACHE_DIR", Setting::CacheFolder("ccache")),
];

pub const TOOLCHAINS: [Toolchain; 4] = [
    Toolchain {
        ecosystem: "rust",
        markers: &["Cargo.toml", "Cargo.lock"],
        // rustup's proxies in `~/.cargo/bin` run the toolchains `~/.rustup` holds.
        install_roots: &[&["~/.cargo/bin", "~/.rustup"]],
        variables: &[
            ("CARGO_HOME", Setting::CacheFolder("cargo")),
            ("SCCACHE_DIR", Setting::CacheFolder("sccache")),
            // A wrapper the owner configured would reach a cache of the owner's.
            ("RUSTC_WRAPPER", Setting::Value("")),
            ("RUSTC_WORKSPACE_WRAPPER", Setting::Value("")),
            ("CARGO_BUILD_RUSTC_WRAPPER", Setting::Value("")),
        ],
        registry_hosts: &["index.crates.io", "static.crates.io"],
    },
    Toolchain {
        ecosystem: "node",
        markers: &["package.json", "package-lock.json"],
        install_roots: &[
            &["~/.nvm/versions/node/v*"],
            &["~/.volta"],
            &["~/.bun/bin"],
            &["~/.deno/bin"],
        ],
        variables: &[
            ("npm_config_cache", Setting::CacheFolder("npm")),
            ("npm_config_userconfig", Setting::ConfigFile("npmrc")),
        ],
        registry_hosts: &["registry.npmjs.org"],
    },
    Toolchain {
        ecosystem: "python",
        markers: &["pyproject.toml", "requirements.txt"],
        install_roots: &[&["~/.pyenv"]],
        variables: &[
            ("PIP_CACHE_DIR", Setting::CacheFolder("pip")),
            ("UV_CACHE_DIR", Setting::CacheFolder("uv")),
            ("PIP_CONFIG_FILE", Setting::ConfigFile("pip.conf")),
        ],
        registry_hosts: &["pypi.org", "files.pythonhosted.org"],
    },
    Toolchain {
        ecosystem: "go",
        markers: &["go.mod", "go.sum"],
        install_roots: &[&["~/go/bin"], &["~/.goenv"]],
        variables: &[
            ("GOMODCACHE", Setting::CacheFolder("go-mod")),
            ("GOCACHE", Setting::CacheFolder("go-build")),
            // The module cache is read-only by default, which leaves a session's cache
            // impossible to remove at its end.
            ("GOFLAGS", Setting::Flag("-modcacherw")),
        ],
        // `sum.golang.org` is Go's default checksum database, asked about any module
        // `go.sum` does not list yet.
        registry_hosts: &["proxy.golang.org", "sum.golang.org"],
    },
];

/// The ecosystems the checkout uses, in table order: those with a marker file at its root.
pub fn detect(checkout: &Path) -> io::Result<Vec<&'static Toolchain>> {
    let mut detected = Vec::new();
    for toolchain in &TOOLCHAINS {
        for marker in toolchain.markers {
            if is_file(&checkout.join(marker))? {
                detected.push(toolchain);
                break;
            }
        }
    }
    Ok(detected)
}

fn is_file(path: &Path) -> io::Result<bool> {
    match path.metadata() {
        Ok(metadata) => Ok(metadata.is_file()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io::Error::new(
            error.kind(),
            format!("cannot read {}: {error}", path.display()),
        )),
    }
}

impl Toolchain {
    /// The folders to bind read-only for a launch whose `PATH` holds `path_entries`, with
    /// `home` the agent's home: each install root group whose first root holds one of the
    /// entries. A `PATH` entry outside every group, such as `~/.local/bin`, binds nothing.
    /// Refuses a root of the table that is not a [`HomePath`], or a companion root with `*`.
    pub fn roots_to_bind(
        &self,
        home: &Path,
        path_entries: &[PathBuf],
    ) -> Result<Vec<PathBuf>, InvalidValue> {
        let mut roots = Vec::new();
        for group in self.install_roots {
            let Some((first, companions)) = group.split_first() else {
                continue;
            };
            let first = HomePath::new(*first)?;
            let companions = companions
                .iter()
                .map(|companion| companion_root(home, companion))
                .collect::<Result<Vec<_>, _>>()?;
            for entry in path_entries {
                let Some(root) = root_holding(home, &first, entry) else {
                    continue;
                };
                for root in std::iter::once(root).chain(companions.iter().cloned()) {
                    if !roots.contains(&root) {
                        roots.push(root);
                    }
                }
            }
        }
        Ok(roots)
    }
}

fn companion_root(home: &Path, companion: &str) -> Result<PathBuf, InvalidValue> {
    let companion = HomePath::new(companion)?;
    if companion.as_str().contains('*') {
        return Err(InvalidValue {
            kind: "install root",
            value: companion.to_string(),
            rule: "only the first root of a group may end with `*`",
        });
    }
    Ok(in_home(home, &companion))
}

/// The folder `root` names in `home` when `entry` lies inside it; for a root whose last name
/// holds `*`, the one folder `entry` lies in.
fn root_holding(home: &Path, root: &HomePath, entry: &Path) -> Option<PathBuf> {
    let root = in_home(home, root);
    let parent = root.parent()?;
    let pattern = root.file_name()?.to_str()?;
    let name = entry
        .strip_prefix(parent)
        .ok()?
        .components()
        .next()?
        .as_os_str()
        .to_str()?;
    matches_name(pattern, name).then(|| parent.join(name))
}

fn in_home(home: &Path, path: &HomePath) -> PathBuf {
    path.components()
        .fold(home.to_path_buf(), |folder, name| folder.join(name))
}
