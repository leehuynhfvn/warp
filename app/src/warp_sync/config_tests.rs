use super::*;

const HOME: &str = "/home/dev";

fn resolve(configured: &str) -> Result<PathBuf, WarpSyncError> {
    resolve_mirror_root(configured, Some(Path::new(HOME)))
}

#[test]
fn an_empty_setting_uses_the_default_folder() {
    assert_eq!(resolve("").unwrap(), Path::new("/home/dev/.warp/mirrors"));
    assert_eq!(resolve("  \t").unwrap(), Path::new("/home/dev/.warp/mirrors"));
}

#[test]
fn a_leading_tilde_stands_for_the_home_directory() {
    assert_eq!(resolve("~").unwrap(), Path::new(HOME));
    assert_eq!(resolve("~/sync").unwrap(), Path::new("/home/dev/sync"));
    assert_eq!(resolve("~/a/b").unwrap(), Path::new("/home/dev/a/b"));
}

#[test]
fn an_absolute_path_is_kept() {
    assert_eq!(resolve(" /srv/mirrors ").unwrap(), Path::new("/srv/mirrors"));
}

#[test]
fn relative_paths_and_other_users_homes_are_rejected() {
    for input in ["mirrors", "./mirrors", "../mirrors", "~other/mirrors"] {
        assert!(
            matches!(resolve(input), Err(WarpSyncError::LocalIo(_))),
            "{input}"
        );
    }
}

#[test]
fn a_nul_character_is_rejected() {
    assert!(resolve("/srv/a\0b").is_err());
}

#[test]
fn tilde_and_the_default_need_a_known_home_directory() {
    assert!(resolve_mirror_root("", None).is_err());
    assert!(resolve_mirror_root("~/x", None).is_err());
    assert_eq!(
        resolve_mirror_root("/srv/x", None).unwrap(),
        Path::new("/srv/x")
    );
}

#[test]
fn limits_are_converted_from_mib() {
    let limits = SyncLimits::from_mib(32, 4);

    assert_eq!(limits.max_download_kib, 32 * 1024);
    assert_eq!(limits.max_upload_bytes, 4 * 1024 * 1024);
    assert_eq!(limits, SyncLimits::default());
}

#[test]
fn limits_stay_within_what_is_supported() {
    let too_small = SyncLimits::from_mib(0, 0);
    let too_big = SyncLimits::from_mib(u32::MAX, u32::MAX);

    assert_eq!(too_small, SyncLimits::from_mib(1, 1));
    assert_eq!(
        too_big,
        SyncLimits::from_mib(MAX_CONFIGURABLE_DOWNLOAD_MIB, MAX_CONFIGURABLE_UPLOAD_MIB)
    );
}
