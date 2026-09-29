//! Opens the local mirror, and comparisons with the server, in a VS Code–compatible editor.

use std::ffi::OsString;
use std::io::ErrorKind;
use std::path::PathBuf;

use command::Stdio;
use command::blocking::Command;
use warpui::{AppContext, SingletonEntity};

use super::WarpSyncError;
use crate::util::file::external_editor::Editor;
use crate::util::file::external_editor::settings::{EditorChoice, EditorSettings};

/// How many files of one comparison are opened side by side; the report covers the rest.
pub const MAX_EDITOR_DIFFS: usize = 10;

/// The command-line launcher of the editor chosen for opening file links. Only editors whose
/// launcher can open a side-by-side diff qualify.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditorCli {
    program: &'static str,
    name: &'static str,
}

impl EditorCli {
    pub const VS_CODE: Self = Self {
        program: "code",
        name: "VS Code",
    };

    /// The editor chosen for opening file links, or VS Code when that editor cannot show the
    /// mirror.
    pub fn from_settings_or_vs_code(ctx: &AppContext) -> Self {
        Self::from_settings(ctx).unwrap_or(Self::VS_CODE)
    }

    pub fn from_settings(ctx: &AppContext) -> Option<Self> {
        match *EditorSettings::as_ref(ctx).open_file_editor {
            EditorChoice::ExternalEditor(editor) => Self::for_editor(editor),
            EditorChoice::SystemDefault | EditorChoice::Warp | EditorChoice::EnvEditor => None,
        }
    }

    pub fn for_editor(editor: Editor) -> Option<Self> {
        let (program, name) = match editor {
            Editor::VSCode => return Some(Self::VS_CODE),
            Editor::VSCodeInsiders => ("code-insiders", "VS Code Insiders"),
            Editor::Cursor => ("cursor", "Cursor"),
            Editor::Windsurf => ("windsurf", "Windsurf"),
            Editor::PyCharm
            | Editor::PyCharmCE
            | Editor::IntelliJ
            | Editor::IntelliJCE
            | Editor::CLion
            | Editor::CLionCE
            | Editor::RustRoverPreview
            | Editor::RustRover
            | Editor::Atom
            | Editor::WebStorm
            | Editor::PhpStorm
            | Editor::RubyMine
            | Editor::Zed
            | Editor::ZedPreview
            | Editor::GoLand
            | Editor::Rider
            | Editor::DataSpell
            | Editor::DataGrip
            | Editor::AndroidStudio => return None,
            #[cfg(not(target_os = "macos"))]
            Editor::Sublime => return None,
            #[cfg(target_os = "macos")]
            Editor::Sublime4 | Editor::Sublime3 | Editor::Sublime2 => return None,
        };
        Some(Self { program, name })
    }

    pub fn name(&self) -> &'static str {
        self.name
    }
}

/// What to show in the editor. The workspace is always the mirror of a whole host, so that the
/// editor's source control view sees the Git baseline at its root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditorRequest {
    OpenMirror {
        workspace: PathBuf,
        /// A file to open in the workspace.
        file: Option<PathBuf>,
    },
    OpenDiffs {
        workspace: PathBuf,
        /// `(server copy, mirror copy)` pairs, each opened side by side.
        diffs: Vec<(PathBuf, PathBuf)>,
        /// Files opened on their own.
        files: Vec<PathBuf>,
    },
}

/// The argument lists to run the launcher with, in order. Every path is absolute, so none can be
/// taken for an option.
pub fn invocations(request: &EditorRequest) -> Vec<Vec<OsString>> {
    match request {
        EditorRequest::OpenMirror { workspace, file } => {
            vec![
                std::iter::once(workspace)
                    .chain(file)
                    .map(OsString::from)
                    .collect(),
            ]
        }
        EditorRequest::OpenDiffs {
            workspace,
            diffs,
            files,
        } => {
            let mut invocations = vec![vec![OsString::from(workspace)]];
            invocations.extend(diffs.iter().map(|(server, mirror)| {
                vec![
                    OsString::from("-r"),
                    OsString::from("--diff"),
                    OsString::from(server),
                    OsString::from(mirror),
                ]
            }));
            if !files.is_empty() {
                invocations.push(
                    std::iter::once(OsString::from("-r"))
                        .chain(files.iter().map(OsString::from))
                        .collect(),
                );
            }
            invocations
        }
    }
}

/// Runs the launcher for `request`. Blocks until every invocation has handed its request to the
/// editor, which the launchers do without waiting for the editor to close.
pub fn launch(cli: EditorCli, request: &EditorRequest) -> Result<(), WarpSyncError> {
    for args in invocations(request) {
        let status = Command::new(cli.program)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|err| match err.kind() {
                ErrorKind::NotFound => WarpSyncError::Editor(format!(
                    "`{}` was not found; install the {} command-line launcher and make sure it \
                     is on PATH",
                    cli.program, cli.name
                )),
                _ => WarpSyncError::Editor(format!("could not run `{}`: {err}", cli.program)),
            })?;
        if !status.success() {
            return Err(WarpSyncError::Editor(format!(
                "`{}` exited with {status}",
                cli.program
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "editor_tests.rs"]
mod tests;
