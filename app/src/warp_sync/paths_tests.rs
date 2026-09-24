use super::*;

fn normalize(input: &str, pwd: Option<&str>) -> Result<String, WarpSyncError> {
    normalize_remote_path(input, pwd)
}

#[test]
fn absolute_path_is_kept() {
    assert_eq!(normalize("/etc/hostname", None).unwrap(), "/etc/hostname");
}

#[test]
fn surrounding_whitespace_is_trimmed() {
    assert_eq!(normalize("  /etc/hostname\t", None).unwrap(), "/etc/hostname");
}

#[test]
fn relative_path_is_joined_with_pwd() {
    assert_eq!(
        normalize("nginx/nginx.conf", Some("/etc")).unwrap(),
        "/etc/nginx/nginx.conf"
    );
}

#[test]
fn relative_path_without_pwd_is_rejected() {
    assert!(matches!(
        normalize("nginx.conf", None),
        Err(WarpSyncError::InvalidPath(_))
    ));
}

#[test]
fn relative_path_with_relative_pwd_is_rejected() {
    assert!(matches!(
        normalize("nginx.conf", Some("etc")),
        Err(WarpSyncError::InvalidPath(_))
    ));
}

#[test]
fn duplicate_slashes_and_dot_segments_are_collapsed() {
    assert_eq!(normalize("/a//b/./c", None).unwrap(), "/a/b/c");
}

#[test]
fn trailing_slash_is_removed() {
    assert_eq!(normalize("/etc/nginx/", None).unwrap(), "/etc/nginx");
}

#[test]
fn parent_segments_are_rejected() {
    assert!(matches!(
        normalize("/etc/../shadow", None),
        Err(WarpSyncError::InvalidPath(_))
    ));
    assert!(matches!(
        normalize("../shadow", Some("/etc")),
        Err(WarpSyncError::InvalidPath(_))
    ));
}

#[test]
fn tilde_paths_are_rejected() {
    assert!(matches!(
        normalize("~/notes", None),
        Err(WarpSyncError::InvalidPath(_))
    ));
}

#[test]
fn root_is_rejected() {
    assert!(matches!(normalize("/", None), Err(WarpSyncError::InvalidPath(_))));
    assert!(matches!(normalize("//", None), Err(WarpSyncError::InvalidPath(_))));
    assert!(matches!(normalize("/.", None), Err(WarpSyncError::InvalidPath(_))));
}

#[test]
fn empty_input_is_rejected() {
    assert!(matches!(normalize("", None), Err(WarpSyncError::InvalidPath(_))));
    assert!(matches!(normalize("  ", None), Err(WarpSyncError::InvalidPath(_))));
}

#[test]
fn pseudo_filesystems_are_rejected() {
    for path in ["/proc", "/proc/1/environ", "/sys/kernel", "/dev/null", "/run/secrets"] {
        assert!(
            matches!(normalize(path, None), Err(WarpSyncError::InvalidPath(_))),
            "{path} should be rejected"
        );
    }
}

#[test]
fn pseudo_filesystem_prefix_match_is_by_component() {
    assert_eq!(normalize("/procedures/a", None).unwrap(), "/procedures/a");
    assert_eq!(normalize("/system/a", None).unwrap(), "/system/a");
}

#[test]
fn control_characters_are_rejected() {
    assert!(matches!(
        normalize("/etc/a\nb", None),
        Err(WarpSyncError::InvalidPath(_))
    ));
    assert!(matches!(
        normalize("/etc/a\0b", None),
        Err(WarpSyncError::InvalidPath(_))
    ));
}

#[test]
fn overlong_path_is_rejected() {
    let path = format!("/{}", "a".repeat(MAX_REMOTE_PATH_LEN));
    assert!(matches!(
        normalize(&path, None),
        Err(WarpSyncError::InvalidPath(_))
    ));
}

#[test]
fn shell_metacharacters_are_preserved_verbatim() {
    assert_eq!(
        normalize("/tmp/a b'c$d`e`", None).unwrap(),
        "/tmp/a b'c$d`e`"
    );
}

#[test]
fn host_key_keeps_safe_characters() {
    assert_eq!(host_key("prod-1.example.com"), "prod-1.example.com");
}

#[test]
fn host_key_replaces_unsafe_characters() {
    assert_eq!(host_key("my host!"), "my_host_");
    assert_eq!(host_key("a/b"), "a_b");
}

#[test]
fn host_key_of_empty_hostname_is_placeholder() {
    assert_eq!(host_key(""), "unknown-host");
}

#[test]
fn host_key_cannot_escape_or_collide_with_manifest_dir() {
    assert_eq!(host_key(".."), "_.");
    assert_eq!(host_key("."), "_");
    assert_eq!(host_key(".warp-sync"), "_warp-sync");
}

#[test]
fn split_parent_name_of_nested_path() {
    assert_eq!(
        split_parent_name("/etc/nginx/nginx.conf"),
        ("/etc/nginx".to_owned(), "nginx.conf".to_owned())
    );
}

#[test]
fn split_parent_name_of_top_level_path() {
    assert_eq!(
        split_parent_name("/etc"),
        ("/".to_owned(), "etc".to_owned())
    );
}

#[test]
fn local_path_mirrors_remote_layout() {
    let root = Path::new("/home/me/.warp/mirrors");
    assert_eq!(
        local_path_for(root, "prod-1", "/etc/nginx/nginx.conf"),
        PathBuf::from("/home/me/.warp/mirrors/prod-1/etc/nginx/nginx.conf")
    );
}

#[test]
fn manifest_path_is_per_host_under_hidden_dir() {
    let root = Path::new("/m");
    assert_eq!(
        manifest_path(root, "prod-1"),
        PathBuf::from("/m/.warp-sync/prod-1.json")
    );
}

#[test]
fn staging_dirs_are_unique_and_under_hidden_dir() {
    let root = Path::new("/m");
    let first = staging_dir(root);
    let second = staging_dir(root);
    assert_ne!(first, second);
    assert!(first.starts_with("/m/.warp-sync/staging"));
}
