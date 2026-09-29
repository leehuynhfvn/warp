use settings::macros::define_settings_group;
use settings::{RespectUserSyncSetting, SupportedPlatforms, SyncToCloud};

/// A language to highlight files with, for the files whose path matches `glob`.
#[derive(
    Debug,
    serde::Serialize,
    serde::Deserialize,
    PartialEq,
    Eq,
    Clone,
    schemars::JsonSchema,
    settings_value::SettingsValue,
)]
pub struct LanguageOverride {
    /// `*` matches any run of characters, `/` included; `?` matches one character. Files opened
    /// from a server with Warp Sync are matched by their path on the server.
    pub glob: String,
    /// One of the language names Warp supports, e.g. `ini`, `shell`, `nginx`.
    pub language: String,
}

define_settings_group!(CodeSettings, settings: [
    code_as_default_editor: CodeAsDefaultEditor {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Never,
        surface: settings::SettingSurfaces::GUI,
        private: false,
        toml_path: "code.editor.use_warp_as_default_editor",
        description: "Whether Warp is used as the default code editor.",
    }
    codebase_context_enabled: CodebaseContextEnabled {
        type: bool,
        default: true,
        supported_platforms: SupportedPlatforms::DESKTOP,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        surface: settings::SettingSurfaces::GUI,
        private: false,
        storage_key: "AgentModeCodebaseContext",
        toml_path: "code.indexing.agent_mode_codebase_context",
        description: "Whether codebase context is provided to the AI agent.",
    },
    auto_indexing_enabled: AutoIndexingEnabled {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::DESKTOP,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        surface: settings::SettingSurfaces::GUI,
        private: false,
        storage_key: "AgentModeCodebaseContextAutoIndexing",
        toml_path: "code.indexing.agent_mode_codebase_context_auto_indexing",
        description: "Whether automatic codebase indexing is enabled.",
    },
    // Whether or not the user has manually dismissed the code toolbelt new feature popup.
    dismissed_code_toolbelt_new_feature_popup: DismissedCodeToolbeltNewFeaturePopup {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        surface: settings::SettingSurfaces::GUI,
        private: true,
    },
    // Controls whether the project explorer / file tree appears in the tools panel.
    show_project_explorer: ShowProjectExplorer {
        type: bool,
        default: true,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        surface: settings::SettingSurfaces::GUI,
        private: false,
        toml_path: "code.editor.show_project_explorer",
        description: "Whether the project explorer is shown in the tools panel.",
    },
    // Controls whether global file search appears in the tools panel.
    show_global_search: ShowGlobalSearch {
        type: bool,
        default: true,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        surface: settings::SettingSurfaces::GUI,
        private: false,
        toml_path: "code.editor.show_global_search",
        description: "Whether global file search is shown in the tools panel.",
    },
    // Controls whether hidden files (dotfiles) are shown in the project explorer.
    show_hidden_files: ShowHiddenFiles {
        type: bool,
        default: true,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        surface: settings::SettingSurfaces::GUI,
        private: false,
        toml_path: "code.editor.show_hidden_files",
        description: "Whether hidden files (dotfiles) are shown in the project explorer.",
    },
    // Controls whether the language server reformats the file on save.
    format_on_save: FormatOnSave {
        type: bool,
        default: true,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        surface: settings::SettingSurfaces::GUI,
        private: false,
        toml_path: "code.editor.format_on_save",
        description: "Whether the language server automatically formats the file on save. Other LSP features (hover, go-to-definition, references, diagnostics) are unaffected.",
    },
    // Controls whether the Warp text editor automatically saves file changes as the
    // user types (debounced) and when the editor loses focus. Only applies to the
    // Warp text editor, not the command line or AI input.
    auto_save: AutoSave {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        surface: settings::SettingSurfaces::GUI,
        private: false,
        toml_path: "code.editor.auto_save",
        description: "Whether the Warp text editor automatically saves changes as you type and when the editor loses focus.",
    },
    // Languages chosen for files that Warp does not recognize, or recognizes wrongly. The first
    // matching entry wins; the language picker in the editor footer adds entries at the front.
    language_overrides: LanguageOverrides {
        type: Vec<LanguageOverride>,
        default: Vec::new(),
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Never,
        surface: settings::SettingSurfaces::GUI,
        private: false,
        toml_path: "code.editor.language_overrides",
        description: "Languages to highlight files with, by path glob, e.g. { glob = \"/etc/haproxy/*.cfg\", language = \"ini\" }.",
    },
]);
