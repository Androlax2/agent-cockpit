//! The Git settings every launch applies over every configuration file Git reads, the
//! checkout's own included (#694): an agent's commit, tag or push is never signed, so never
//! with the owner's key, whatever the owner's or the repository's configuration says.

use std::ffi::OsString;

use crate::Refusal;

/// The signing settings reset, as Git's command-scope configuration
/// (`GIT_CONFIG_COUNT`), which wins over the system, global, local and worktree files.
pub(crate) const UNSIGNED_GIT: &[(&str, &str)] = &[
    ("commit.gpgSign", "false"),
    ("tag.gpgSign", "false"),
    ("tag.forceSignAnnotated", "false"),
    ("push.gpgSign", "false"),
];

const COUNT_VARIABLE: &str = "GIT_CONFIG_COUNT";

/// Where `git -c` passes its settings to the Git programs it runs; read after the
/// `GIT_CONFIG_COUNT` pairs, so the reset is appended there too when the launch holds it.
const PARAMETERS_VARIABLE: &str = "GIT_CONFIG_PARAMETERS";

/// `environment` with [`UNSIGNED_GIT`] appended after the `GIT_CONFIG_COUNT` pairs it holds,
/// which stay as they are, and after its `GIT_CONFIG_PARAMETERS`, when it holds that. Refuses a
/// count Git would refuse too, rather than drop the pairs it names.
pub(crate) fn with_unsigned_git(
    mut environment: Vec<(OsString, OsString)>,
) -> Result<Vec<(OsString, OsString)>, Refusal> {
    let held = environment
        .iter()
        .rev()
        .find(|(name, _)| name == COUNT_VARIABLE)
        .map(|(_, value)| value.to_string_lossy().into_owned());
    let held = match held.as_deref().map(str::parse::<usize>) {
        None => 0,
        Some(Ok(count)) => count,
        Some(Err(_)) => {
            return Err(Refusal::new(
                COUNT_VARIABLE,
                format!(
                    "the launch environment's {COUNT_VARIABLE} is {:?}, not a number of Git \
                     settings",
                    held.unwrap_or_default()
                ),
            ));
        }
    };
    environment.retain(|(name, _)| name != COUNT_VARIABLE);
    for (offset, (key, value)) in UNSIGNED_GIT.iter().enumerate() {
        let index = held + offset;
        environment.push((format!("GIT_CONFIG_KEY_{index}").into(), (*key).into()));
        environment.push((format!("GIT_CONFIG_VALUE_{index}").into(), (*value).into()));
    }
    environment.push((
        COUNT_VARIABLE.into(),
        (held + UNSIGNED_GIT.len()).to_string().into(),
    ));
    let parameters = environment
        .iter()
        .rev()
        .find(|(name, _)| name == PARAMETERS_VARIABLE)
        .map(|(_, value)| value.clone());
    if let Some(mut parameters) = parameters {
        environment.retain(|(name, _)| name != PARAMETERS_VARIABLE);
        for (key, value) in UNSIGNED_GIT {
            parameters.push(format!(" '{key}'='{value}'"));
        }
        environment.push((PARAMETERS_VARIABLE.into(), parameters));
    }
    Ok(environment)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn variable(environment: &[(OsString, OsString)], name: &str) -> Option<String> {
        environment
            .iter()
            .rev()
            .find(|(variable, _)| variable == name)
            .map(|(_, value)| value.to_string_lossy().into_owned())
    }

    #[test]
    fn signing_is_reset_from_the_first_index_when_the_launch_names_no_setting() {
        let environment = with_unsigned_git(Vec::new()).unwrap();

        assert_eq!(
            variable(&environment, "GIT_CONFIG_COUNT").as_deref(),
            Some("4")
        );
        assert_eq!(
            variable(&environment, "GIT_CONFIG_KEY_0").as_deref(),
            Some("commit.gpgSign")
        );
        assert_eq!(
            variable(&environment, "GIT_CONFIG_VALUE_0").as_deref(),
            Some("false")
        );
    }

    #[test]
    fn the_own_settings_of_a_launch_are_kept_before_the_reset() {
        let environment = with_unsigned_git(vec![
            ("GIT_CONFIG_COUNT".into(), "1".into()),
            ("GIT_CONFIG_KEY_0".into(), "core.editor".into()),
            ("GIT_CONFIG_VALUE_0".into(), "vi".into()),
        ])
        .unwrap();

        assert_eq!(
            variable(&environment, "GIT_CONFIG_KEY_0").as_deref(),
            Some("core.editor")
        );
        assert_eq!(
            variable(&environment, "GIT_CONFIG_KEY_1").as_deref(),
            Some("commit.gpgSign")
        );
        assert_eq!(
            variable(&environment, "GIT_CONFIG_COUNT").as_deref(),
            Some("5")
        );
        let counts = environment
            .iter()
            .filter(|(name, _)| name == "GIT_CONFIG_COUNT")
            .count();
        assert_eq!(counts, 1);
    }

    #[test]
    fn the_reset_comes_after_the_settings_git_c_passed() {
        let environment = with_unsigned_git(vec![(
            "GIT_CONFIG_PARAMETERS".into(),
            "'commit.gpgsign'='true'".into(),
        )])
        .unwrap();

        assert_eq!(
            variable(&environment, "GIT_CONFIG_PARAMETERS").as_deref(),
            Some(
                "'commit.gpgsign'='true' 'commit.gpgSign'='false' 'tag.gpgSign'='false' \
                 'tag.forceSignAnnotated'='false' 'push.gpgSign'='false'"
            )
        );
    }

    #[test]
    fn a_count_that_is_no_number_is_refused() {
        let refused = with_unsigned_git(vec![("GIT_CONFIG_COUNT".into(), "two".into())]);

        assert!(refused.is_err());
    }
}
