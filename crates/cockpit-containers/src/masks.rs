//! Torlek's masks helper (#390, option C): `torlek-sandbox engine-masks`, run as root by
//! `torlek-engine-masks.socket`, shows a project's engine one checkout through the session
//! baseline's masks. A session reaches its project's engine only once the helper has shown
//! that engine the session's checkout masked.

use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

use crate::EngineUser;

/// The helper's socket unit; each instance of `torlek-engine-masks@.service` answers one
/// connection.
pub const MASKS_SOCKET_UNIT: &str = "torlek-engine-masks.socket";

/// Where `torlek-engine-masks.socket` listens.
pub const MASKS_SOCKET: &str = "/run/torlek-engine-masks.sock";

/// Where each engine's setup records the engine for the helper, one `<user>.conf` each.
pub const ENGINES_DIR: &str = "/etc/torlek/engines";

/// How long the helper may take to mask a checkout.
pub const MASKS_DEADLINE: Duration = Duration::from_secs(10);

/// The helper's answer is one short line.
const MAX_ANSWER_BYTES: u64 = 4096;

/// The helper, as the daemon reaches it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MasksHelper {
    pub socket: PathBuf,
    /// The user the kernel must name as the socket's listener: root, for the installed one.
    pub uid: u32,
    /// Where each engine's setup records the engine for the helper: `ENGINES_DIR`.
    pub engines_dir: PathBuf,
}

/// Why no session reaches a verified engine yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MasksUnready {
    /// Torlek's masks helper is not installed, with why.
    NotInstalled(String),
    /// The helper is installed, but the engine's setup has not given it to the owner or
    /// recorded the engine for it: running that setup again does.
    NotSetUp(String),
}

impl MasksHelper {
    /// The helper the units install, listening as root.
    pub fn installed() -> MasksHelper {
        MasksHelper {
            socket: PathBuf::from(MASKS_SOCKET),
            uid: 0,
            engines_dir: PathBuf::from(ENGINES_DIR),
        }
    }

    /// Asks the helper to show `user`'s engine `checkout` masked, within `deadline`. Nothing
    /// is asked of a listener the kernel does not name as the helper's user.
    pub async fn show_masked(
        &self,
        user: &EngineUser,
        checkout: &Path,
        deadline: Duration,
    ) -> Result<(), String> {
        if checkout.as_os_str().as_bytes().contains(&b'\n') {
            return Err(format!(
                "{} holds a newline, which the helper's request cannot",
                checkout.display()
            ));
        }
        let shown = self.socket.display();
        match tokio::time::timeout(deadline, self.ask(user, checkout)).await {
            Ok(answer) => answer.map_err(|reason| format!("{shown}: {reason}")),
            Err(_) => Err(format!(
                "{shown}: no answer within {}s",
                deadline.as_secs_f32()
            )),
        }
    }

    async fn ask(&self, user: &EngineUser, checkout: &Path) -> Result<(), String> {
        let mut stream = UnixStream::connect(&self.socket)
            .await
            .map_err(|error| format!("cannot connect: {error}"))?;
        let listener_uid = stream
            .peer_cred()
            .map_err(|error| format!("the kernel named no listener: {error}"))?
            .uid();
        if listener_uid != self.uid {
            return Err(format!(
                "uid {listener_uid} listens, not Torlek's masks helper (uid {})",
                self.uid
            ));
        }
        let mut request = format!("{} ", user.name()).into_bytes();
        request.extend_from_slice(checkout.as_os_str().as_bytes());
        request.push(b'\n');
        stream
            .write_all(&request)
            .await
            .map_err(|error| format!("cannot ask: {error}"))?;
        let mut answer = Vec::new();
        (&mut stream)
            .take(MAX_ANSWER_BYTES)
            .read_to_end(&mut answer)
            .await
            .map_err(|error| format!("cannot read the answer: {error}"))?;
        let answer = String::from_utf8_lossy(&answer);
        let line = answer.strip_suffix('\n').unwrap_or(&answer);
        if line == "ok" {
            return Ok(());
        }
        match line.strip_prefix("refused: ") {
            Some(reason) => Err(format!("the helper refused: {reason}")),
            None => Err(format!("the helper answered {line:?}")),
        }
    }

    /// Whether the helper would show `user`'s engine a checkout for `owner_uid`, the one user
    /// its setup lets connect: a cheap look for readiness, which asks the helper nothing.
    pub fn check_ready(&self, owner_uid: u32, user: &EngineUser) -> Result<(), MasksUnready> {
        let shown = self.socket.display();
        let metadata = match std::fs::symlink_metadata(&self.socket) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(MasksUnready::NotInstalled(format!(
                    "Torlek's masks helper is not installed: no {shown}"
                )));
            }
            Err(error) => {
                return Err(MasksUnready::NotInstalled(format!(
                    "cannot read {shown}: {error}"
                )));
            }
        };
        if !metadata.file_type().is_socket() {
            return Err(MasksUnready::NotInstalled(format!(
                "{shown} is not a socket"
            )));
        }
        if metadata.uid() != owner_uid {
            return Err(MasksUnready::NotSetUp(format!(
                "{shown} lets only uid {} connect, not yours ({owner_uid})",
                metadata.uid()
            )));
        }
        let record = self.engines_dir.join(format!("{}.conf", user.name()));
        match std::fs::symlink_metadata(&record) {
            Ok(_) => Ok(()),
            Err(error) => Err(MasksUnready::NotSetUp(format!(
                "the helper has no record of this engine: {}: {error}",
                record.display()
            ))),
        }
    }
}
