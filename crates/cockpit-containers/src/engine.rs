//! The engine a project gets (#390, #856 Q2 and Q3): rootless Docker under a system user of
//! its own, one per project, run by a root-installed unit, answering on a socket in that
//! unit's runtime folder.

use std::fmt;
use std::path::{Path, PathBuf};

/// The system unit template every project's engine runs from; its instance is the user.
pub const UNIT_TEMPLATE: &str = "torlek-engine@.service";

/// Where the system keeps the accounts the engine users are looked up in.
pub const PASSWD_FILE: &str = "/etc/passwd";

/// `useradd` refuses longer names.
const MAX_USER_NAME_BYTES: usize = 32;

/// A project's engine user: `torlek-e<project id>`, or `torlek-<profile>-e<project id>` in
/// another profile. Project names do not fit a Linux user name (64 characters, upper case,
/// dots), so the user is named after the project's id in the profile's state.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EngineUser(String);

impl EngineUser {
    /// `qualified_profile` is the profile's qualified name (`torlek`, `torlek-next`).
    pub fn for_project(qualified_profile: &str, project_id: i64) -> Result<EngineUser, String> {
        if project_id < 1 {
            return Err(format!("project id {project_id} names no project"));
        }
        let name = format!("{qualified_profile}-e{project_id}");
        let is_shaped = name.starts_with(|first: char| first.is_ascii_lowercase())
            && name.chars().all(|letter| {
                letter.is_ascii_lowercase() || letter.is_ascii_digit() || letter == '-'
            });
        if !is_shaped {
            return Err(format!(
                "{name:?} is not a user name: lowercase letters, digits and hyphens only"
            ));
        }
        if name.len() > MAX_USER_NAME_BYTES {
            return Err(format!(
                "the engine user {name} is longer than {MAX_USER_NAME_BYTES} characters, \
                 which useradd refuses; a shorter profile name fits"
            ));
        }
        Ok(EngineUser(name))
    }

    pub fn name(&self) -> &str {
        &self.0
    }

    /// `torlek-engine@<user>.service`.
    pub fn unit(&self) -> String {
        format!("torlek-engine@{}.service", self.0)
    }

    /// The unit's runtime folder, its `XDG_RUNTIME_DIR`.
    pub fn runtime_dir(&self) -> PathBuf {
        Path::new("/run").join(&self.0)
    }

    /// Where rootless `dockerd` listens: `$XDG_RUNTIME_DIR/docker.sock`.
    pub fn socket(&self) -> PathBuf {
        self.runtime_dir().join("docker.sock")
    }

    /// The user's home, which holds the engine's images and containers.
    pub fn home(&self) -> PathBuf {
        Path::new("/var/lib").join(&self.0)
    }
}

impl fmt::Display for EngineUser {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// One line of the accounts file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub name: String,
    pub uid: u32,
    pub home: PathBuf,
}

/// The account named `name` in `passwd`, the text of [`PASSWD_FILE`]. Engine users are local
/// accounts `useradd` wrote there, so no other name service is asked.
pub fn account_named(passwd: &str, name: &str) -> Option<Account> {
    accounts(passwd).find(|account| account.name == name)
}

/// The first account with `uid` in `passwd`.
pub fn account_with_uid(passwd: &str, uid: u32) -> Option<Account> {
    accounts(passwd).find(|account| account.uid == uid)
}

/// `name:password:uid:gid:gecos:home:shell`; a line of another shape is skipped.
fn accounts(passwd: &str) -> impl Iterator<Item = Account> + '_ {
    passwd.lines().filter_map(|line| {
        let fields: Vec<&str> = line.split(':').collect();
        let [name, _, uid, _, _, home, _] = fields[..] else {
            return None;
        };
        Some(Account {
            name: name.to_owned(),
            uid: uid.parse().ok()?,
            home: PathBuf::from(home),
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_profile_names_the_user_after_the_project_id() {
        let user = EngineUser::for_project("torlek", 3).unwrap();

        assert_eq!(user.name(), "torlek-e3");
        assert_eq!(user.unit(), "torlek-engine@torlek-e3.service");
        assert_eq!(user.socket(), Path::new("/run/torlek-e3/docker.sock"));
        assert_eq!(user.home(), Path::new("/var/lib/torlek-e3"));
    }

    #[test]
    fn another_profile_names_itself_in_the_user() {
        let user = EngineUser::for_project("torlek-next", 12).unwrap();

        assert_eq!(user.name(), "torlek-next-e12");
    }

    #[test]
    fn a_name_longer_than_useradd_takes_is_refused() {
        let error = EngineUser::for_project("torlek-abcdefghijklmnopqrstuvwx", 1234).unwrap_err();

        assert!(error.contains("longer than 32"), "{error}");
    }

    #[test]
    fn a_profile_that_is_not_a_user_name_is_refused() {
        assert!(EngineUser::for_project("Torlek", 1).is_err());
        assert!(EngineUser::for_project("torlek", 0).is_err());
    }

    #[test]
    fn accounts_are_found_by_name_and_by_uid() {
        let passwd = "root:x:0:0::/root:/bin/bash\n\
                      # a comment\n\
                      theo:x:1000:1000:Theo:/home/theo:/usr/bin/fish\n\
                      torlek-e3:x:961:961::/var/lib/torlek-e3:/usr/bin/nologin\n";

        assert_eq!(account_named(passwd, "torlek-e3").unwrap().uid, 961);
        assert_eq!(
            account_with_uid(passwd, 1000).unwrap().home,
            Path::new("/home/theo")
        );
        assert_eq!(account_named(passwd, "torlek-e4"), None);
    }
}
