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
    assert_eq!(
        normalize("  /etc/hostname\t", None).unwrap(),
        "/etc/hostname"
    );
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
    assert!(matches!(
        normalize("/", None),
        Err(WarpSyncError::InvalidPath(_))
    ));
    assert!(matches!(
        normalize("//", None),
        Err(WarpSyncError::InvalidPath(_))
    ));
    assert!(matches!(
        normalize("/.", None),
        Err(WarpSyncError::InvalidPath(_))
    ));
}

#[test]
fn empty_input_is_rejected() {
    assert!(matches!(
        normalize("", None),
        Err(WarpSyncError::InvalidPath(_))
    ));
    assert!(matches!(
        normalize("  ", None),
        Err(WarpSyncError::InvalidPath(_))
    ));
}

#[test]
fn pseudo_filesystems_are_rejected() {
    for path in [
        "/proc",
        "/proc/1/environ",
        "/sys/kernel",
        "/dev/null",
        "/run/secrets",
    ] {
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
fn host_key_of_an_altered_name_is_disambiguated_by_a_hash() {
    let key = host_key("my host!");

    assert!(key.starts_with("my_host_-"), "{key}");
    assert_eq!(key.len(), "my_host_-".len() + 8);
    assert_eq!(key, host_key("my host!"));
}

#[test]
fn hostnames_that_sanitize_alike_get_different_keys() {
    let keys = [
        host_key("prod_db"),
        host_key("prod/db"),
        host_key("prod db"),
        host_key("prod\ndb"),
    ];

    let unique: std::collections::BTreeSet<_> = keys.iter().collect();
    assert_eq!(unique.len(), keys.len(), "{keys:?}");
    assert_eq!(keys[0], "prod_db");
}

#[test]
fn host_key_of_empty_hostname_is_a_hashed_placeholder() {
    assert!(host_key("").starts_with("unknown-host-"));
}

#[test]
fn host_key_cannot_escape_or_collide_with_manifest_dir() {
    for hostname in ["..", ".", ".warp-sync", "...", ".hidden"] {
        let key = host_key(hostname);
        assert!(!key.starts_with('.'), "{hostname} -> {key}");
        assert!(!key.contains('/'), "{hostname} -> {key}");
        assert_ne!(key, ".warp-sync");
    }
    assert!(host_key("..").starts_with("unknown-host-"));
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

#[test]
fn selection_is_normalized_against_the_block_pwd() {
    assert_eq!(
        selection_to_remote_path(Some("  nginx.conf "), Some("/etc")).unwrap(),
        "/etc/nginx.conf"
    );
}

#[test]
fn empty_or_missing_selection_is_rejected() {
    for selection in [None, Some(""), Some("   ")] {
        assert!(matches!(
            selection_to_remote_path(selection, Some("/etc")),
            Err(WarpSyncError::InvalidPath(_))
        ));
    }
}

#[test]
fn multi_line_selection_is_rejected() {
    assert!(matches!(
        selection_to_remote_path(Some("/etc/a\n/etc/b"), None),
        Err(WarpSyncError::InvalidPath(_))
    ));
}

#[test]
fn recovery_dirs_are_unique_and_under_hidden_dir() {
    let root = Path::new("/m");

    let first = recovery_dir(root);

    assert_ne!(first, recovery_dir(root));
    assert!(first.starts_with("/m/.warp-sync/recovered"));
}

#[cfg(unix)]
#[test]
fn private_dirs_are_only_accessible_to_the_owner() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let nested = dir.path().join("a/b");

    create_private_dir_all(&nested).unwrap();
    create_private_dir_all(&nested).unwrap();

    for path in [dir.path().join("a"), nested] {
        let mode = fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }
}

#[test]
fn machine_host_keys_differ_per_machine_and_extend_the_host_key() {
    let first = machine_host_key("draff3", "aaaaaaaaaaaaaaaa");
    let second = machine_host_key("draff3", "bbbbbbbbbbbbbbbb");

    assert_ne!(first, second);
    assert!(first.starts_with("draff3-"));
    assert_eq!(first, machine_host_key("draff3", "aaaaaaaaaaaaaaaa"));
}

#[test]
fn diff_path_lives_in_the_state_directory_and_uses_a_safe_name() {
    let path = diff_path(Path::new("/m"), "prod-1", "/etc/nginx/my conf's.d");

    assert_eq!(
        path,
        Path::new("/m/.warp-sync/diffs/prod-1/etc_nginx_my_conf_s.d.diff")
    );
}

#[test]
fn diff_path_names_are_bounded_and_never_empty() {
    let long = format!("/{}", "a".repeat(1000));

    let long_name = diff_path(Path::new("/m"), "h", &long);
    let root_name = diff_path(Path::new("/m"), "h", "/");

    assert!(long_name.file_name().unwrap().len() <= MAX_DIFF_STEM_CHARS + ".diff".len());
    assert_eq!(root_name, Path::new("/m/.warp-sync/diffs/h/root.diff"));
}

#[test]
fn printable_escapes_control_characters_only() {
    assert_eq!(printable("a\nb\tc\u{1b}[0m"), "a\\nb\\tc\\u{1b}[0m");
    assert_eq!(printable("héllo wörld/'x'"), "héllo wörld/'x'");
}

#[test]
fn git_metadata_cannot_be_synced() {
    for input in ["/.git", "/srv/repo/.git", "/srv/repo/.git/config"] {
        assert!(
            matches!(
                normalize_remote_path(input, None),
                Err(WarpSyncError::InvalidPath(_))
            ),
            "{input}"
        );
    }
    assert_eq!(
        normalize_remote_path("/etc/.gitignore", None).unwrap(),
        "/etc/.gitignore"
    );
}

#[test]
fn names_that_some_filesystem_resolves_to_git_metadata_are_recognized() {
    for name in [
        ".git",
        ".GIT",
        ".Git",
        ".git.",
        ".git ",
        ".git::$INDEX_ALLOCATION",
        "GIT~1",
        ".g\u{200c}it",
        ".git\u{feff}",
    ] {
        assert!(is_git_metadata_name(name), "{name:?}");
    }
    for name in [".gitignore", ".gitattributes", "git", ".github", "a.git"] {
        assert!(!is_git_metadata_name(name), "{name:?}");
    }
}

#[test]
fn a_mirror_folder_belongs_to_the_host_of_that_name_or_its_machine_suffixed_variant() {
    let suffixed = machine_host_key(&host_key("prod-1"), "0123456789abcdef");

    assert!(host_dir_matches("prod-1", "prod-1"));
    assert!(host_dir_matches(&suffixed, "prod-1"));
    assert!(!host_dir_matches("prod-2", "prod-1"));
    assert!(!host_dir_matches("prod-1-backup", "prod-1"));
    assert!(!host_dir_matches("prod-1-ab12cd3", "prod-1"));
    assert!(!host_dir_matches("prod-1-ab12cd3g", "prod-1"));
    assert!(!host_dir_matches("prod-1x", "prod-1"));
}

#[test]
fn a_host_whose_name_had_to_be_altered_matches_its_hashed_folder() {
    let folder = host_key("my host!");

    assert!(host_dir_matches(&folder, "my host!"));
    assert!(!host_dir_matches(&folder, "other"));
}

#[test]
fn printable_escapes_invisible_and_direction_changing_characters() {
    assert_eq!(printable("a\u{202E}b"), "a\\u{202e}b");
    assert_eq!(printable("a\u{200B}b"), "a\\u{200b}b");
    assert_eq!(printable("a\u{2028}b"), "a\\u{2028}b");
    assert_eq!(printable("a\u{FEFF}b"), "a\\u{feff}b");
    assert_eq!(
        printable("caf\u{e9} \u{4e2d}\u{6587}"),
        "caf\u{e9} \u{4e2d}\u{6587}"
    );
}
