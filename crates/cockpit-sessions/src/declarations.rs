//! The sandbox's rules for what a program may declare (#861), checked when the daemon loads
//! its providers: a declaration is refused there, whole, rather than narrowed at a launch.

use std::sync::Arc;

use cockpit_provider_api::{Credentials, HomePath, InvalidValue, Needs, Provider};
use cockpit_sandbox::{BASELINE_DENIED_READ, BASELINE_DENIED_WRITE};

/// One provider whose declaration the sandbox refuses, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefusedDeclaration {
    pub provider: String,
    pub reason: String,
}

/// The providers among `providers` whose sandbox declaration is refused: a malformed one; one
/// that reads or writes a path the baseline never shows or never lets change
/// (`BASELINE_DENIED_READ`, `BASELINE_DENIED_WRITE`); and one that reads or writes a loaded
/// provider's sign-in state, a folder it declares `write` or a credential file of its, which is
/// readable by that provider alone.
pub fn refused_declarations(providers: &[Arc<dyn Provider>]) -> Vec<RefusedDeclaration> {
    let mut refused = Vec::new();
    let mut loaded = Vec::new();
    for provider in providers {
        let name = provider.name().as_str().to_owned();
        let reason = match declaration(provider.as_ref()) {
            Err(error) => Some(format!("its sandbox declaration is invalid: {error}")),
            Ok(declaration) => match baseline_overlap(&declaration.0) {
                Some(reason) => Some(reason),
                None => {
                    loaded.push((name.clone(), declaration));
                    None
                }
            },
        };
        if let Some(reason) = reason {
            refused.push(RefusedDeclaration {
                provider: name,
                reason,
            });
        }
    }
    for (name, (needs, _)) in &loaded {
        if let Some(reason) = foreign_state_overlap(name, needs, &loaded) {
            refused.push(RefusedDeclaration {
                provider: name.clone(),
                reason,
            });
        }
    }
    refused
}

fn declaration(provider: &dyn Provider) -> Result<(Needs, Credentials), InvalidValue> {
    Ok((provider.needs()?, provider.credentials()?))
}

fn baseline_overlap(needs: &Needs) -> Option<String> {
    let baseline = baseline_home_paths();
    declared_paths(needs).find_map(|path| {
        let denied = baseline.iter().find(|denied| path.overlaps(denied))?;
        Some(format!(
            "it declares {path}, which overlaps {denied}: the sandbox never shows or never lets \
             a program change that path"
        ))
    })
}

/// Why `needs`, of the provider `name`, reaches another loaded provider's sign-in state.
fn foreign_state_overlap(
    name: &str,
    needs: &Needs,
    loaded: &[(String, (Needs, Credentials))],
) -> Option<String> {
    let foreign_state = loaded.iter().filter(|(other, _)| other != name).flat_map(
        |(other, (other_needs, credentials))| {
            other_needs
                .write
                .iter()
                .chain(&credentials.files)
                .map(move |state| (other, state))
        },
    );
    for (other, state) in foreign_state {
        if let Some(path) = declared_paths(needs).find(|path| path.overlaps(state)) {
            return Some(format!(
                "it declares {path}, which overlaps {state}, sign-in state of {other}: a \
                 program's sign-in state is readable by it alone"
            ));
        }
    }
    None
}

fn declared_paths(needs: &Needs) -> impl Iterator<Item = &HomePath> {
    needs.read.iter().chain(&needs.write)
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
        Capabilities, LaunchCommand, LaunchRequest, ProviderError, ProviderName,
    };

    use super::*;

    /// A provider that declares `read`, `write` and `credential_files`, and is never
    /// launched.
    struct Declaring {
        name: ProviderName,
        read: &'static [&'static str],
        write: &'static [&'static str],
        credential_files: &'static [&'static str],
        credential_bindings: &'static [(&'static str, &'static [&'static str])],
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

        fn credentials(&self) -> Result<Credentials, InvalidValue> {
            Credentials::new(
                self.credential_files,
                self.credential_bindings,
                &self.needs()?,
            )
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
            credential_files: &[],
            credential_bindings: &[],
        })
    }

    fn signed_in(name: &str, credential_files: &'static [&'static str]) -> Arc<dyn Provider> {
        Arc::new(Declaring {
            name: ProviderName::new(name).unwrap(),
            read: &[],
            write: &[],
            credential_files,
            credential_bindings: &[],
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
                .contains("~/.shared, sign-in state of owner"),
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

    #[test]
    fn another_providers_credential_file_is_refused_to_the_one_that_reads_it() {
        let providers = [
            signed_in("owner", &["~/.config/owner-login"]),
            declaring("reader", &["~/.config/owner-login"], &[]),
        ];

        assert_eq!(refused_names(&providers), ["reader"]);
    }

    #[test]
    fn a_refused_providers_folders_refuse_no_other() {
        let providers = [
            declaring("refused", &["~/.aws"], &["~/.refused"]),
            declaring("reader", &["~/.refused/state"], &[]),
        ];

        assert_eq!(refused_names(&providers), ["refused"]);
    }

    #[test]
    fn a_malformed_credential_declaration_is_refused() {
        let providers: [Arc<dyn Provider>; 1] = [Arc::new(Declaring {
            name: ProviderName::new("agent").unwrap(),
            read: &[],
            write: &[],
            credential_files: &[],
            credential_bindings: &[("TOKEN", &["undeclared.example.com"])],
        })];

        let refused = refused_declarations(&providers);

        assert_eq!(refused.len(), 1, "{refused:?}");
        assert!(refused[0].reason.contains("invalid"), "{refused:?}");
    }
}
