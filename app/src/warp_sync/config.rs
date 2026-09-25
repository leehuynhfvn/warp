//! The locations and limits that the user can configure for Warp Sync.

use std::path::{Path, PathBuf};

use settings::Setting as _;
use warpui::{AppContext, SingletonEntity};

use super::{
    DEFAULT_MAX_DOWNLOAD_MIB, DEFAULT_MAX_UPLOAD_MIB, MAX_CONFIGURABLE_DOWNLOAD_MIB,
    MAX_CONFIGURABLE_UPLOAD_MIB, WarpSyncError,
};
use crate::settings::WarpSyncSettings;

const KIB_PER_MIB: u64 = 1024;
const BYTES_PER_MIB: usize = 1024 * 1024;
const HOME_PREFIX: &str = "~";

/// Size limits of a single transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncLimits {
    /// Upper bound on the remote size (`du -sk`) of a download.
    pub max_download_kib: u64,
    /// Upper bound on the compressed size of an upload.
    pub max_upload_bytes: usize,
}

impl SyncLimits {
    /// Builds limits from MiB values, keeping each within what Warp Sync supports.
    pub fn from_mib(max_download_mib: u32, max_upload_mib: u32) -> Self {
        let download = max_download_mib.clamp(1, MAX_CONFIGURABLE_DOWNLOAD_MIB);
        let upload = max_upload_mib.clamp(1, MAX_CONFIGURABLE_UPLOAD_MIB);
        Self {
            max_download_kib: u64::from(download) * KIB_PER_MIB,
            max_upload_bytes: upload as usize * BYTES_PER_MIB,
        }
    }
}

impl Default for SyncLimits {
    fn default() -> Self {
        Self::from_mib(DEFAULT_MAX_DOWNLOAD_MIB, DEFAULT_MAX_UPLOAD_MIB)
    }
}

/// Where mirrors live and how much a transfer may move.
#[derive(Debug, Clone)]
pub struct SyncConfig {
    pub mirror_root: PathBuf,
    pub limits: SyncLimits,
}

impl SyncConfig {
    pub fn from_settings(app: &AppContext) -> Result<Self, WarpSyncError> {
        let settings = WarpSyncSettings::as_ref(app);
        Ok(Self {
            mirror_root: resolve_mirror_root(
                settings.mirror_root.value(),
                dirs::home_dir().as_deref(),
            )?,
            limits: SyncLimits::from_mib(
                *settings.max_download_mib.value(),
                *settings.max_upload_mib.value(),
            ),
        })
    }
}

/// Turns the configured mirror folder into an absolute path. An empty setting means the default
/// (`~/.warp/mirrors`); a leading `~` stands for the home directory.
pub fn resolve_mirror_root(configured: &str, home: Option<&Path>) -> Result<PathBuf, WarpSyncError> {
    let configured = configured.trim();
    if configured.contains('\0') {
        return Err(invalid_mirror_root("it contains a NUL character"));
    }
    if configured.is_empty() {
        return home
            .map(|home| home.join(".warp").join("mirrors"))
            .ok_or_else(home_unknown);
    }
    if let Some(rest) = configured.strip_prefix(HOME_PREFIX)
        && (rest.is_empty() || rest.starts_with('/'))
    {
        let home = home.ok_or_else(home_unknown)?;
        return Ok(home.join(rest.trim_start_matches('/')));
    }
    let path = PathBuf::from(configured);
    if !path.is_absolute() {
        return Err(invalid_mirror_root(
            "use an absolute path, or one that starts with ~/",
        ));
    }
    Ok(path)
}

fn invalid_mirror_root(reason: &str) -> WarpSyncError {
    WarpSyncError::LocalIo(format!("invalid mirror folder: {reason}"))
}

fn home_unknown() -> WarpSyncError {
    WarpSyncError::LocalIo("the home directory could not be determined".to_owned())
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
