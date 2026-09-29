//! Remote files that are being edited in Warp's own editor. Each one was downloaded into the
//! mirror through a live session, and a manual save uploads it back through that same session.
//!
//! The table only lives in memory: after a restart the files stay in the mirror and can still be
//! uploaded with Warp Sync, but a save never goes to the server through a session other than the
//! one that opened the file.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use warp_core::features::FeatureFlag;
use warpui::{Entity, ModelContext, SingletonEntity, WindowId};

use super::paths::{normalize_remote_path, printable};
use crate::terminal::model::session::SessionId;

/// Whether remote files can be opened in Warp's editor.
pub fn is_enabled() -> bool {
    FeatureFlag::WarpSync.is_enabled() && FeatureFlag::WarpSyncRemoteEdit.is_enabled()
}

/// The remote path that `token`, a word of a remote block's output, names, resolved against the
/// block's working directory `pwd`. Nothing can be checked on the server while hovering, so only
/// words that look like paths qualify: absolute ones, and relative ones with a `/` or a file
/// extension. Numbers, versions, addresses, options and URLs do not.
pub fn remote_link_path(token: &str, pwd: &str) -> Option<String> {
    let looks_like_path = !token.is_empty()
        && !token.chars().any(|c| c.is_whitespace() || c.is_control())
        && !token.starts_with('-')
        && !token.contains("://")
        && (token.starts_with('/')
            || token.contains('/')
            || (token.contains('.') && token.chars().any(char::is_alphabetic)));
    if !looks_like_path {
        return None;
    }
    normalize_remote_path(token, Some(pwd))
        .ok()
        .filter(|path| path != "/")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteEditState {
    /// The mirror copy matches what was last downloaded or uploaded.
    Clean,
    /// The mirror copy was saved but not uploaded.
    Unsynced,
    /// An upload is being prepared, confirmed or sent.
    Uploading,
    Failed(String),
}

/// How an upload of an edited file ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UploadResult {
    Uploaded,
    Cancelled,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteEditFile {
    /// The session that downloaded the file; uploads go through it only.
    pub session_id: SessionId,
    /// The window that opened the file, which reports its uploads.
    pub window_id: WindowId,
    pub remote_path: String,
    pub remote_user: String,
    pub hostname: String,
    state: RemoteEditState,
    /// Whether the file was saved again while an upload of an earlier version was under way.
    saved_during_upload: bool,
}

impl RemoteEditFile {
    pub fn new(
        session_id: SessionId,
        window_id: WindowId,
        remote_path: String,
        remote_user: String,
        hostname: String,
    ) -> Self {
        Self {
            session_id,
            window_id,
            remote_path,
            remote_user,
            hostname,
            state: RemoteEditState::Clean,
            saved_during_upload: false,
        }
    }

    pub fn state(&self) -> &RemoteEditState {
        &self.state
    }

    /// Records a save of the mirror copy and returns whether it has to be uploaded now. Auto-saves
    /// never upload: they would push half-finished edits to the server.
    fn on_saved(&mut self, auto_saved: bool) -> bool {
        match (&self.state, auto_saved) {
            (RemoteEditState::Uploading, _) => {
                self.saved_during_upload = true;
                false
            }
            (
                RemoteEditState::Clean | RemoteEditState::Unsynced | RemoteEditState::Failed(_),
                true,
            ) => {
                self.state = RemoteEditState::Unsynced;
                false
            }
            (
                RemoteEditState::Clean | RemoteEditState::Unsynced | RemoteEditState::Failed(_),
                false,
            ) => {
                self.state = RemoteEditState::Uploading;
                true
            }
        }
    }

    fn on_upload_finished(&mut self, result: UploadResult) {
        let saved_during_upload = std::mem::take(&mut self.saved_during_upload);
        self.state = match result {
            UploadResult::Uploaded if saved_during_upload => RemoteEditState::Unsynced,
            UploadResult::Uploaded => RemoteEditState::Clean,
            UploadResult::Cancelled => RemoteEditState::Unsynced,
            UploadResult::Failed(message) => RemoteEditState::Failed(message),
        };
    }

    /// Where the file lives on the server and what saving it does, for the editor's footer.
    pub fn status_line(&self) -> String {
        let state = match &self.state {
            RemoteEditState::Clean => "Save uploads to the server".to_owned(),
            RemoteEditState::Unsynced => "Unsynced changes".to_owned(),
            RemoteEditState::Uploading => "Uploading…".to_owned(),
            RemoteEditState::Failed(message) => format!("Upload failed: {message}"),
        };
        format!(
            "{}@{}:{} · {state}",
            printable(&self.remote_user),
            printable(&self.hostname),
            printable(&self.remote_path)
        )
    }
}

#[derive(Debug, Clone)]
pub enum RemoteEditEvent {
    /// A file was saved on purpose and its mirror copy has to be uploaded through `session_id`.
    UploadRequested {
        window_id: WindowId,
        local_path: PathBuf,
        session_id: SessionId,
        remote_path: String,
    },
    StateChanged {
        local_path: PathBuf,
    },
}

#[derive(Default)]
pub struct RemoteEditModel {
    /// Keyed by the mirror path the file was opened with.
    open: HashMap<PathBuf, RemoteEditFile>,
}

impl Entity for RemoteEditModel {
    type Event = RemoteEditEvent;
}

impl SingletonEntity for RemoteEditModel {}

impl RemoteEditModel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, local_path: &Path) -> Option<&RemoteEditFile> {
        self.open.get(local_path)
    }

    /// Starts tracking `local_path`, replacing whatever session opened it before.
    pub fn register(
        &mut self,
        local_path: PathBuf,
        file: RemoteEditFile,
        ctx: &mut ModelContext<Self>,
    ) {
        self.open.insert(local_path.clone(), file);
        ctx.emit(RemoteEditEvent::StateChanged { local_path });
    }

    pub fn file_saved(
        &mut self,
        local_path: &Path,
        auto_saved: bool,
        ctx: &mut ModelContext<Self>,
    ) {
        let Some(file) = self.open.get_mut(local_path) else {
            return;
        };
        if file.on_saved(auto_saved) {
            ctx.emit(RemoteEditEvent::UploadRequested {
                window_id: file.window_id,
                local_path: local_path.to_owned(),
                session_id: file.session_id,
                remote_path: file.remote_path.clone(),
            });
        }
        ctx.emit(RemoteEditEvent::StateChanged {
            local_path: local_path.to_owned(),
        });
    }

    pub fn upload_finished(
        &mut self,
        local_path: &Path,
        result: UploadResult,
        ctx: &mut ModelContext<Self>,
    ) {
        let Some(file) = self.open.get_mut(local_path) else {
            return;
        };
        file.on_upload_finished(result);
        ctx.emit(RemoteEditEvent::StateChanged {
            local_path: local_path.to_owned(),
        });
    }
}

#[cfg(test)]
#[path = "remote_edit_tests.rs"]
mod tests;
