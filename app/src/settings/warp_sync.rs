use settings::macros::define_settings_group;
use settings::{SupportedPlatforms, SyncToCloud};

use crate::warp_sync::{DEFAULT_MAX_DOWNLOAD_MIB, DEFAULT_MAX_UPLOAD_MIB};

define_settings_group!(WarpSyncSettings,
    settings: [
        mirror_root: WarpSyncMirrorRoot {
            type: String,
            default: String::new(),
            supported_platforms: SupportedPlatforms::DESKTOP,
            sync_to_cloud: SyncToCloud::Never,
            surface: settings::SettingSurfaces::GUI,
            private: false,
            storage_key: "WarpSyncMirrorRoot",
            toml_path: "warp_sync.mirror_root",
            description: "The folder that holds local mirrors of files downloaded from remote hosts with Warp Sync. Leave empty to use ~/.warp/mirrors. Existing mirrors are not moved.",
        },
        max_download_mib: WarpSyncMaxDownloadMib {
            type: u32,
            default: DEFAULT_MAX_DOWNLOAD_MIB,
            supported_platforms: SupportedPlatforms::DESKTOP,
            sync_to_cloud: SyncToCloud::Never,
            surface: settings::SettingSurfaces::GUI,
            private: false,
            storage_key: "WarpSyncMaxDownloadMib",
            toml_path: "warp_sync.max_download_mib",
            description: "The largest amount of data, in MiB on the remote host, that a single Warp Sync download may transfer.",
        },
        max_upload_mib: WarpSyncMaxUploadMib {
            type: u32,
            default: DEFAULT_MAX_UPLOAD_MIB,
            supported_platforms: SupportedPlatforms::DESKTOP,
            sync_to_cloud: SyncToCloud::Never,
            surface: settings::SettingSurfaces::GUI,
            private: false,
            storage_key: "WarpSyncMaxUploadMib",
            toml_path: "warp_sync.max_upload_mib",
            description: "The largest compressed size, in MiB, that a single Warp Sync upload may have.",
        },
    ]
);
