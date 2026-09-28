//! Paired agent identities: which agent tokens the user has approved (mục 3.11 of the O2 plan).
//! Only a token's SHA-256 is ever stored on disk, never the secret itself. `parse`/`serialize`/
//! `find`/`next_id` are pure so they can be tested without touching disk; `load`/`add`/`forget_all`
//! read and write `~/.warp/agent-ops/agents.toml`.

use std::io::Write as _;
use std::path::Path;
use std::{fs, io};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::AGENTS_FILE;
use crate::warp_sync::paths::create_private_dir_all;

/// One agent the user has approved pairing for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PairedAgent {
    pub(crate) id: String,
    pub(crate) token_sha256: String,
    pub(crate) paired_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentsFile {
    #[serde(default)]
    agents: Vec<PairedAgent>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum PairingError {
    #[error("could not parse it: {0}")]
    Parse(String),
    #[error("could not write it: {0}")]
    Io(String),
}

/// Lowercase hex SHA-256 of `secret`, the only form of an [`crate::agent_bridge`] agent token that
/// is ever written to disk or compared against what is on disk.
pub(crate) fn hash(secret: &str) -> String {
    let digest = Sha256::digest(secret.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Parses `agents.toml`'s contents.
pub(crate) fn parse(text: &str) -> Result<Vec<PairedAgent>, PairingError> {
    let file: AgentsFile =
        toml::from_str(text).map_err(|err| PairingError::Parse(err.to_string()))?;
    Ok(file.agents)
}

fn serialize(agents: &[PairedAgent]) -> Result<String, PairingError> {
    let file = AgentsFile {
        agents: agents.to_vec(),
    };
    toml::to_string_pretty(&file).map_err(|err| PairingError::Parse(err.to_string()))
}

pub(crate) fn find<'a>(agents: &'a [PairedAgent], token_sha256: &str) -> Option<&'a PairedAgent> {
    agents
        .iter()
        .find(|agent| agent.token_sha256 == token_sha256)
}

/// Picks an id for a newly paired agent named `name`: `name` itself if no paired agent already
/// uses it, otherwise `"{name}-2"`, `"{name}-3"`, and so on.
pub(crate) fn next_id(existing: &[PairedAgent], name: &str) -> String {
    if !existing.iter().any(|agent| agent.id == name) {
        return name.to_owned();
    }
    let mut suffix = 2;
    loop {
        let candidate = format!("{name}-{suffix}");
        if !existing.iter().any(|agent| agent.id == candidate) {
            return candidate;
        }
        suffix += 1;
    }
}

/// Loads the paired agents from `home`'s `~/.warp/agent-ops/agents.toml`. A missing, unreadable,
/// or malformed file behaves like an empty list rather than failing: pairing carries no privilege
/// on its own (mục 2.4 of the plan), it only labels who sent a request, so it is safe — and better
/// for the user — to fail open here rather than to deny every write the way an unreadable policy
/// file does.
pub(crate) fn load(home: &Path) -> Vec<PairedAgent> {
    let path = home.join(AGENTS_FILE);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Vec::new(),
        Err(err) => {
            log::warn!("[Agent Bridge] could not read {}: {err}", path.display());
            return Vec::new();
        }
    };
    match parse(&text) {
        Ok(agents) => agents,
        Err(err) => {
            log::warn!("[Agent Bridge] could not parse {}: {err}", path.display());
            Vec::new()
        }
    }
}

/// Adds a newly approved agent and returns its assigned entry. Pairing is additive: an unreadable
/// existing file is treated as empty (mục of [`load`]) rather than blocking the new pairing, so a
/// corrupt file's previous entries would be lost on the next `add` — an accepted trade-off, since
/// forgetting a pairing is exactly what "Agent Ops: Forget all paired agents" is for anyway.
pub(crate) fn add(
    home: &Path,
    name: &str,
    token_sha256: String,
) -> Result<PairedAgent, PairingError> {
    let mut agents = load(home);
    let agent = PairedAgent {
        id: next_id(&agents, name),
        token_sha256,
        paired_at: Utc::now(),
    };
    agents.push(agent.clone());
    write(home, &agents)?;
    Ok(agent)
}

/// Empties the paired-agents file ("Agent Ops: Forget all paired agents").
pub(crate) fn forget_all(home: &Path) -> Result<(), PairingError> {
    write(home, &[])
}

/// Writes `agents` atomically: a temp file in the same directory, then a rename over the real
/// path, so a reader (or a crash mid-write) never sees a half-written file.
fn write(home: &Path, agents: &[PairedAgent]) -> Result<(), PairingError> {
    let path = home.join(AGENTS_FILE);
    let dir = path.parent().unwrap_or(&path);
    create_private_dir_all(dir)
        .map_err(|err| PairingError::Io(format!("could not create {}: {err}", dir.display())))?;

    let text = serialize(agents)?;
    let tmp_path = dir.join(".agents.toml.tmp");
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp_path).map_err(|err| {
        PairingError::Io(format!("could not create {}: {err}", tmp_path.display()))
    })?;
    file.write_all(text.as_bytes()).map_err(|err| {
        PairingError::Io(format!("could not write {}: {err}", tmp_path.display()))
    })?;
    drop(file);
    fs::rename(&tmp_path, &path)
        .map_err(|err| PairingError::Io(format!("could not replace {}: {err}", path.display())))
}

#[cfg(test)]
#[path = "pairing_tests.rs"]
mod tests;
