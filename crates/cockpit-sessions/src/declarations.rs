//! The sandbox's rules for what a program may declare (#861), checked when the daemon loads
//! its providers: a declaration is refused there, whole, rather than narrowed at a launch.

use std::sync::Arc;

use cockpit_provider_api::{HomePath, Needs, Provider};
use cockpit_sandbox::{BASELINE_DENIED_READ, BASELINE_DENIED_WRITE};

/// One provider whose declaration the sandbox refuses, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefusedDeclaration {
    pub provider: String,
    pub reason: String,
}

/// The providers among `providers` whose sandbox declaration is refused: a malformed one; one
/// that reads or writes a path the baseline never shows or never lets change
/// (`BASELINE_DENIED_READ`, `BASELINE_DENIED_WRITE`); and one that reads or writes a folder
/// another provider declares `write`, a program's sign-in state, readable by it alone.
pub fn refused_declarations(providers: &[Arc<dyn Provider>]) -> Vec<RefusedDeclaration> {
    let mut refused = Vec::new();
    let mut declared = Vec::new();
    for provider in providers {
        let name = provider.name().as_str().to_owned();
        match provider.needs() {
            Ok(needs) => declared.push((name, needs)),
            Err(error) => refused.push(RefusedDeclaration {
                provider: name,
                reason: format!("its sandbox declaration is invalid: {error}"),
            }),
        }
    }
    let baseline = baseline_home_paths();
    for (name, needs) in &declared {
        let others_writes =
            declared
                .iter()
                .filter(|(other, _)| other != name)
                .flat_map(|(other, other_needs)| {
                    other_needs.write.iter().map(move |path| (other, path))
                });
        let reason =
            declared_paths(needs)
                .find_map(|path| {
                    baseline.iter().find(|denied| overlaps(path, denied)).map(|denied| {
                    format!(
                        "it declares {path}, which overlaps {denied}: the sandbox never shows \
                         or never lets a program change that path"
                    )
                })
                })
                .or_else(|| {
                    others_writes.into_iter().find_map(|(other, written)| {
                        declared_paths(needs)
                        .find(|path| overlaps(path, written))
                        .map(|path| {
                            format!(
                                "it declares {path}, which overlaps {written}, a folder {other} \
                                 declares write: a program's sign-in state is readable by it \
                                 alone"
                            )
                        })
                    })
                });
        if let Some(reason) = reason {
            refused.push(RefusedDeclaration {
                provider: name.clone(),
                reason,
            });
        }
    }
    refused
}

fn declared_paths(needs: &Needs) -> impl Iterator<Item = &HomePath> {
    needs.read.iter().chain(&needs.write)
}

/// Whether binding one of `first` and `second` exposes the other.
fn overlaps(first: &HomePath, second: &HomePath) -> bool {
    first.covers(second) || second.covers(first)
}

/// The home paths of the baseline denials, a folder's `/**` read as the folder itself.
fn baseline_home_paths() -> Vec<HomePath> {
    BASELINE_DENIED_READ
        .iter()
        .chain(BASELINE_DENIED_WRITE)
        .filter(|rule| rule.starts_with("~/"))
        .map(|rule| {
            let path = rule.strip_suffix("/**").unwrap_or(rule);
            HomePath::new(path).unwrap_or_else(|error| {
                unreachable!("baseline denial {rule} is a home path: {error}")
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use cockpit_provider_api::{
        Capabilities, InvalidValue, LaunchCommand, LaunchRequest, ProviderError, ProviderName,
    };

    use super::*;

    /// A provider that declares `read` and `write`, and is never launched.
    struct Declaring {
        name: ProviderName,
        read: &'static [&'static str],
        write: &'static [&'static str],
    }

    impl Provider for Declaring {
        fn name(&self) -> &ProviderName {
            &self.name
        }

        fn capabilities(&self) -> Capabilities {
            Capabilities::none()
        }

        fn launch(&self, _request: &LaunchRequest<'_>) -> Result<LaunchCommand, ProviderError> {
            unreachable!("not launched here")
        }

        fn needs(&self) -> Result<Needs, InvalidValue> {
            Needs::new(&[], self.read, self.write)
        }
    }

    fn declaring(
        name: &str,
        read: &'static [&'static str],
        write: &'static [&'static str],
    ) -> Arc<dyn Provider> {
        Arc::new(Declaring {
            name: ProviderName::new(name).unwrap(),
            read,
            write,
        })
    }

    fn refused_names(providers: &[Arc<dyn Provider>]) -> Vec<String> {
        refused_declarations(providers)
            .into_iter()
            .map(|refused| refused.provider)
            .collect()
    }

    #[test]
    fn disjoint_declarations_are_accepted() {
        let providers = [
            declaring("first", &["~/.local/share/first"], &["~/.first"]),
            declaring("second", &["~/.local/share/second"], &["~/.second"]),
        ];

        assert_eq!(refused_names(&providers), Vec::<String>::new());
    }

    #[test]
    fn a_path_inside_or_around_a_baseline_denial_is_refused() {
        let cases: [(&'static [&'static str], &'static [&'static str]); 5] = [
            (&["~/.ssh/id_ed25519"], &[]),
            (&["~/.gnupg/private-keys-v1.d"], &[]),
            (&[], &["~/.local/bin"]),
            (&["~/.config"], &[]),
            (&[], &["~/.git-credentials"]),
        ];
        for (read, write) in cases {
            let refused = refused_declarations(&[declaring("agent", read, write)]);

            assert_eq!(refused.len(), 1, "{read:?} {write:?}");
            assert!(
                refused[0].reason.contains("never shows or never lets"),
                "{refused:?}"
            );
        }
    }

    #[test]
    fn a_folder_another_provider_writes_is_refused_to_the_one_that_reads_it() {
        let providers = [
            declaring("owner", &[], &["~/.shared"]),
            declaring("reader", &["~/.shared/state"], &[]),
        ];

        let refused = refused_declarations(&providers);

        assert_eq!(refused.len(), 1, "{refused:?}");
        assert_eq!(refused[0].provider, "reader");
        assert!(
            refused[0]
                .reason
                .contains("~/.shared, a folder owner declares write"),
            "{refused:?}"
        );
    }

    #[test]
    fn two_providers_writing_one_folder_are_both_refused() {
        let providers = [
            declaring("first", &[], &["~/.shared"]),
            declaring("second", &[], &["~/.shared"]),
        ];

        assert_eq!(refused_names(&providers), ["first", "second"]);
    }
}
