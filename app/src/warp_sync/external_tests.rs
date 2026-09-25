use std::fs;

use super::*;

/// A mirror with `prod-1/etc/nginx/nginx.conf` in it.
struct Mirror {
    dir: tempfile::TempDir,
}

impl Mirror {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("mirror/prod-1/etc/nginx")).unwrap();
        fs::write(dir.path().join("mirror/prod-1/etc/nginx/nginx.conf"), "x").unwrap();
        fs::create_dir_all(dir.path().join("mirror/.warp-sync/compare")).unwrap();
        Self { dir }
    }

    fn root(&self) -> PathBuf {
        self.dir.path().join("mirror")
    }

    fn resolve(&self, relative: &str) -> Result<MirrorPath, WarpSyncError> {
        let path = self.root().join(relative);
        resolve_mirror_path(&self.root(), &path.to_string_lossy())
    }
}

fn mapped(host: &str, remote: Option<&str>) -> Result<MirrorPath, WarpSyncError> {
    Ok(MirrorPath {
        host_dir_name: host.to_owned(),
        remote_path: remote.map(str::to_owned),
    })
}

#[test]
fn a_file_maps_to_its_host_and_remote_path() {
    let mirror = Mirror::new();

    assert_eq!(
        mirror.resolve("prod-1/etc/nginx/nginx.conf"),
        mapped("prod-1", Some("/etc/nginx/nginx.conf"))
    );
}

#[test]
fn a_folder_maps_like_a_file_even_with_a_trailing_slash() {
    let mirror = Mirror::new();

    assert_eq!(
        mirror.resolve("prod-1/etc/nginx/"),
        mapped("prod-1", Some("/etc/nginx"))
    );
}

#[test]
fn the_host_folder_itself_has_no_remote_path() {
    let mirror = Mirror::new();

    assert_eq!(mirror.resolve("prod-1"), mapped("prod-1", None));
}

#[test]
fn a_path_that_does_not_exist_yet_is_accepted_below_an_existing_host_folder() {
    let mirror = Mirror::new();

    assert_eq!(
        mirror.resolve("prod-1/var/log/app"),
        mapped("prod-1", Some("/var/log/app"))
    );
}

#[test]
fn a_new_host_folder_is_not_created_by_a_client() {
    let mirror = Mirror::new();

    assert!(matches!(
        mirror.resolve("other-host/etc"),
        Err(WarpSyncError::InvalidPath(_))
    ));
}

#[test]
fn the_mirror_root_itself_is_refused() {
    let mirror = Mirror::new();

    assert!(matches!(
        resolve_mirror_path(&mirror.root(), &mirror.root().to_string_lossy()),
        Err(WarpSyncError::InvalidPath(_))
    ));
}

#[test]
fn paths_outside_the_mirror_are_refused() {
    let mirror = Mirror::new();
    let outside = mirror.dir.path().join("elsewhere");
    fs::create_dir_all(&outside).unwrap();

    for path in [
        outside.to_string_lossy().into_owned(),
        "/etc/passwd".to_owned(),
        "/".to_owned(),
    ] {
        assert!(
            matches!(
                resolve_mirror_path(&mirror.root(), &path),
                Err(WarpSyncError::InvalidPath(_))
            ),
            "{path}"
        );
    }
}

#[test]
fn a_sibling_that_only_shares_the_root_prefix_is_outside() {
    let mirror = Mirror::new();
    let sibling = mirror.dir.path().join("mirror-evil/prod-1");
    fs::create_dir_all(&sibling).unwrap();

    assert!(matches!(
        resolve_mirror_path(&mirror.root(), &sibling.to_string_lossy()),
        Err(WarpSyncError::InvalidPath(_))
    ));
}

#[test]
fn relative_paths_and_parent_components_are_refused() {
    let mirror = Mirror::new();

    for path in [
        "prod-1/etc".to_owned(),
        "etc/nginx".to_owned(),
        format!("{}/prod-1/../prod-1/etc", mirror.root().display()),
        format!("{}/prod-1/etc/../../..", mirror.root().display()),
        format!("{}/prod-1/new/../../..", mirror.root().display()),
    ] {
        assert!(
            matches!(
                resolve_mirror_path(&mirror.root(), &path),
                Err(WarpSyncError::InvalidPath(_))
            ),
            "{path}"
        );
    }
}

#[test]
fn a_path_with_a_nul_byte_is_refused() {
    let mirror = Mirror::new();
    let path = format!("{}/prod-1/etc\0/x", mirror.root().display());

    assert!(matches!(
        resolve_mirror_path(&mirror.root(), &path),
        Err(WarpSyncError::InvalidPath(_))
    ));
}

#[test]
fn the_state_folder_is_not_a_host() {
    let mirror = Mirror::new();

    for relative in [".warp-sync", ".warp-sync/compare", ".warp-sync/compare/prod-1"] {
        assert!(
            matches!(mirror.resolve(relative), Err(WarpSyncError::InvalidPath(_))),
            "{relative}"
        );
    }
}

#[test]
fn git_metadata_is_never_a_remote_path() {
    let mirror = Mirror::new();
    fs::create_dir_all(mirror.root().join("prod-1/.git")).unwrap();

    for relative in ["prod-1/.git", "prod-1/.git/config", "prod-1/etc/.GIT/x"] {
        assert!(
            matches!(mirror.resolve(relative), Err(WarpSyncError::InvalidPath(_))),
            "{relative}"
        );
    }
}

#[test]
fn names_with_control_characters_are_refused() {
    let mirror = Mirror::new();

    assert!(matches!(
        mirror.resolve("prod-1/etc/a\nb"),
        Err(WarpSyncError::InvalidPath(_))
    ));
}

#[test]
fn names_that_would_be_trimmed_or_hide_characters_are_refused() {
    let mirror = Mirror::new();

    for relative in ["prod-1/etc/trailing ", "prod-1/etc/ leading", "prod-1/etc/tab\t", "prod-1/etc/a\u{7f}b"] {
        assert!(
            matches!(mirror.resolve(relative), Err(WarpSyncError::InvalidPath(_))),
            "{relative:?}"
        );
    }
    assert_eq!(
        mirror.resolve("prod-1/etc/inner space"),
        mapped("prod-1", Some("/etc/inner space"))
    );
}

#[cfg(unix)]
#[test]
fn a_link_that_leads_nowhere_is_refused() {
    use std::os::unix::fs::symlink;

    let mirror = Mirror::new();
    symlink("/nonexistent/outside", mirror.root().join("prod-1/etc/dangling")).unwrap();

    for relative in ["prod-1/etc/dangling", "prod-1/etc/dangling/new"] {
        assert!(
            matches!(mirror.resolve(relative), Err(WarpSyncError::InvalidPath(_))),
            "{relative}"
        );
    }
}

#[cfg(unix)]
#[test]
fn a_symlink_that_leaves_the_mirror_is_refused() {
    use std::os::unix::fs::symlink;

    let mirror = Mirror::new();
    let outside = mirror.dir.path().join("secrets");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("key"), "s").unwrap();
    symlink(&outside, mirror.root().join("prod-1/etc/out")).unwrap();

    for relative in ["prod-1/etc/out", "prod-1/etc/out/key", "prod-1/etc/out/new"] {
        assert!(
            matches!(mirror.resolve(relative), Err(WarpSyncError::InvalidPath(_))),
            "{relative}"
        );
    }
}

#[cfg(unix)]
#[test]
fn a_symlink_to_the_state_folder_is_refused() {
    use std::os::unix::fs::symlink;

    let mirror = Mirror::new();
    symlink(
        mirror.root().join(".warp-sync"),
        mirror.root().join("prod-1/state"),
    )
    .unwrap();

    assert!(matches!(
        mirror.resolve("prod-1/state/compare"),
        Err(WarpSyncError::InvalidPath(_))
    ));
}

#[cfg(unix)]
#[test]
fn a_symlink_that_stays_inside_the_mirror_maps_to_its_target() {
    use std::os::unix::fs::symlink;

    let mirror = Mirror::new();
    symlink(
        mirror.root().join("prod-1/etc/nginx"),
        mirror.root().join("prod-1/nginx-link"),
    )
    .unwrap();

    assert_eq!(
        mirror.resolve("prod-1/nginx-link/nginx.conf"),
        mapped("prod-1", Some("/etc/nginx/nginx.conf"))
    );
}

#[cfg(unix)]
#[test]
fn a_symlinked_mirror_root_is_followed() {
    use std::os::unix::fs::symlink;

    let mirror = Mirror::new();
    let link = mirror.dir.path().join("mirror-link");
    symlink(mirror.root(), &link).unwrap();

    let path = mirror.root().join("prod-1/etc");
    assert_eq!(
        resolve_mirror_path(&link, &path.to_string_lossy()),
        mapped("prod-1", Some("/etc"))
    );
}

#[test]
fn a_missing_mirror_root_is_reported() {
    let dir = tempfile::tempdir().unwrap();

    assert!(matches!(
        resolve_mirror_path(&dir.path().join("nope"), "/tmp/x"),
        Err(WarpSyncError::InvalidPath(_))
    ));
}

fn candidate(
    id: &str,
    hostname: &str,
    window_id: WindowId,
    is_active: bool,
) -> SessionCandidate<&'static str> {
    SessionCandidate {
        session: "session",
        session_id: id.to_owned(),
        hostname: hostname.to_owned(),
        user: "root".to_owned(),
        window_id,
        tab_index: 0,
        is_active,
    }
}

#[test]
fn no_matching_session_asks_the_user_to_open_one() {
    let window = WindowId::new();

    let error = choose_session(vec![candidate("a", "other", window, true)], "prod-1", None)
        .unwrap_err();

    assert_eq!(error, WarpSyncError::NoSession("prod-1".to_owned()));
    assert!(error.to_string().contains("prod-1"));
    assert_eq!(
        choose_session::<&str>(Vec::new(), "prod-1", None).unwrap_err(),
        WarpSyncError::NoSession("prod-1".to_owned())
    );
}

#[test]
fn the_only_matching_session_is_used() {
    let window = WindowId::new();

    let chosen = choose_session(
        vec![
            candidate("a", "other", window, true),
            candidate("b", "prod-1", window, false),
        ],
        "prod-1",
        None,
    )
    .unwrap();

    assert_eq!(chosen.session_id, "b");
}

#[test]
fn a_mirror_folder_with_a_machine_suffix_matches_the_host_name() {
    let window = WindowId::new();
    let suffixed = format!("prod-1-{}", "ab12cd34");

    let chosen =
        choose_session(vec![candidate("a", "prod-1", window, true)], &suffixed, None).unwrap();

    assert_eq!(chosen.session_id, "a");
}

#[test]
fn another_hosts_folder_that_merely_starts_with_the_name_does_not_match() {
    let window = WindowId::new();

    for dir in ["prod-10", "prod-1-backup", "prod-1-ab12cd3", "prod-1-zzzzzzzz"] {
        assert_eq!(
            choose_session(vec![candidate("a", "prod-1", window, true)], dir, None).unwrap_err(),
            WarpSyncError::NoSession(dir.to_owned()),
            "{dir}"
        );
    }
}

#[test]
fn several_sessions_prefer_the_active_one_of_the_focused_window() {
    let focused = WindowId::new();
    let other = WindowId::new();

    let chosen = choose_session(
        vec![
            candidate("a", "prod-1", other, true),
            candidate("b", "prod-1", focused, false),
            candidate("c", "prod-1", focused, true),
        ],
        "prod-1",
        Some(focused),
    )
    .unwrap();

    assert_eq!(chosen.session_id, "c");
}

#[test]
fn several_sessions_without_a_clear_winner_are_ambiguous() {
    let focused = WindowId::new();

    let no_focus = choose_session(
        vec![
            candidate("a", "prod-1", focused, true),
            candidate("b", "prod-1", focused, false),
        ],
        "prod-1",
        None,
    )
    .unwrap_err();
    let two_active = choose_session(
        vec![
            candidate("a", "prod-1", focused, true),
            candidate("b", "prod-1", focused, true),
        ],
        "prod-1",
        Some(focused),
    )
    .unwrap_err();
    let none_active = choose_session(
        vec![
            candidate("a", "prod-1", focused, false),
            candidate("b", "prod-1", focused, false),
        ],
        "prod-1",
        Some(focused),
    )
    .unwrap_err();

    for error in [no_focus, two_active, none_active] {
        let WarpSyncError::AmbiguousSession(message) = error else {
            panic!("expected an ambiguity, got {error:?}");
        };
        assert!(message.contains("session a") && message.contains("session b"));
    }
}

#[test]
fn a_long_list_of_ambiguous_sessions_is_shortened() {
    let window = WindowId::new();
    let candidates = (0..8)
        .map(|i| candidate(&format!("s{i}"), "prod-1", window, false))
        .collect();

    let WarpSyncError::AmbiguousSession(message) =
        choose_session(candidates, "prod-1", None).unwrap_err()
    else {
        panic!("expected an ambiguity");
    };

    assert!(message.contains("and 3 more"), "{message}");
    assert!(!message.contains("session s7"));
}
