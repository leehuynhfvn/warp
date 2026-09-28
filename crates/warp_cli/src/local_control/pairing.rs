//! The client side of pairing (mục 3.11 of the O2 agent-ops policy plan): a persistent token this
//! process presents to Warp in every request so it can be recognized across restarts, once the
//! user has approved pairing it.

use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use local_control::protocol::AgentToken;

const TOKEN_DIR: &str = ".warp/agent-ops/agent-tokens";

#[derive(Debug, thiserror::Error)]
pub(super) enum PairingTokenError {
    #[error("the user's home directory could not be found")]
    NoHomeDirectory,
    #[error("could not create {}: {source}", .path.display())]
    CreateDir { path: PathBuf, source: io::Error },
    #[error("could not read {}: {source}", .path.display())]
    Read { path: PathBuf, source: io::Error },
    #[error("{} is not a valid token", .path.display())]
    Invalid { path: PathBuf },
    #[error(
        "{}'s permissions ({mode:03o}) let the group or other users read or write it; run \
         'chmod 600' on it",
        .path.display()
    )]
    Permissions { path: PathBuf, mode: u32 },
    #[error("could not write {}: {source}", .path.display())]
    Write { path: PathBuf, source: io::Error },
}

/// Loads this client's persistent token for `name`, generating and saving a new one the first
/// time. `name` must already be sanitized to filesystem-safe characters (the same name
/// `agent.pair` is called with).
pub(super) fn token_for(name: &str) -> Result<AgentToken, PairingTokenError> {
    let home = dirs::home_dir().ok_or(PairingTokenError::NoHomeDirectory)?;
    let dir = home.join(TOKEN_DIR);
    let path = dir.join(format!("{name}.token"));

    if let Some(token) = read_token(&path)? {
        return Ok(token);
    }
    let token = AgentToken::generate();
    match write_token(&dir, &path, &token) {
        Ok(()) => Ok(token),
        // Another process created it between our read and our write; use what it wrote.
        Err(PairingTokenError::Write { source, .. }) if source.kind() == io::ErrorKind::AlreadyExists => {
            read_token(&path)?.ok_or(PairingTokenError::Invalid { path })
        }
        Err(err) => Err(err),
    }
}

fn read_token(path: &Path) -> Result<Option<AgentToken>, PairingTokenError> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(PairingTokenError::Read {
                path: path.to_owned(),
                source,
            });
        }
    };
    check_permissions(path, &metadata)?;
    let text = std::fs::read_to_string(path).map_err(|source| PairingTokenError::Read {
        path: path.to_owned(),
        source,
    })?;
    AgentToken::try_from(text.trim().to_owned())
        .map(Some)
        .map_err(|_| PairingTokenError::Invalid {
            path: path.to_owned(),
        })
}

#[cfg(unix)]
fn check_permissions(path: &Path, metadata: &std::fs::Metadata) -> Result<(), PairingTokenError> {
    use std::os::unix::fs::PermissionsExt as _;

    let mode = metadata.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        return Err(PairingTokenError::Permissions {
            path: path.to_owned(),
            mode,
        });
    }
    Ok(())
}

#[cfg(not(unix))]
fn check_permissions(_path: &Path, _metadata: &std::fs::Metadata) -> Result<(), PairingTokenError> {
    Ok(())
}

fn write_token(dir: &Path, path: &Path, token: &AgentToken) -> Result<(), PairingTokenError> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder.create(dir).map_err(|source| PairingTokenError::CreateDir {
        path: dir.to_owned(),
        source,
    })?;

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|source| PairingTokenError::Write {
        path: path.to_owned(),
        source,
    })?;
    file.write_all(token.secret().as_bytes())
        .map_err(|source| PairingTokenError::Write {
            path: path.to_owned(),
            source,
        })
}

#[cfg(test)]
#[path = "pairing_tests.rs"]
mod tests;
