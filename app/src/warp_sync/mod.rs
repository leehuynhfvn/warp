//! Warp Sync: mirrors remote files and directories to a local directory and uploads edits back,
//! using the active session's own command executor so that the transfer runs with the same
//! privileges as the shell the user is looking at.

mod archive;
mod error;
mod manifest;
mod paths;
mod remote_script;

use std::time::Duration;

pub use error::WarpSyncError;

/// Upper bound on the remote size (`du -sk`) of a download. Output travels through the PTY
/// hex-encoded, which doubles its size.
pub const MAX_DOWNLOAD_KIB: u64 = 32 * 1024;

/// Upper bound on the total uncompressed bytes extracted from a downloaded archive.
pub const MAX_EXTRACTED_BYTES: u64 = 256 * 1024 * 1024;

/// Upper bound on the number of entries in a downloaded archive.
pub const MAX_ENTRIES: usize = 20_000;

/// Upper bound on the compressed size of an upload, which has to be typed through the remote
/// shell's line editor.
pub const MAX_UPLOAD_BYTES: usize = 4 * 1024 * 1024;

/// Length of one base64 upload chunk. Must be a multiple of 4 so that every chunk decodes on its
/// own.
pub const UPLOAD_CHUNK_B64_LEN: usize = 16 * 1024;

/// In-band commands only run once the shell is idle, so a stuck shell must not block forever.
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(120);
