use super::*;

/// Points `$HOME` at a temporary, empty directory, runs `body`, then restores `$HOME`. `#[serial]`
/// on the caller keeps this from racing another test's `$HOME`.
fn with_temp_home(body: impl FnOnce(&Path)) {
    let home = tempfile::tempdir().unwrap();
    let previous_home = std::env::var_os("HOME");
    unsafe {
        std::env::set_var("HOME", home.path());
    }
    body(home.path());
    unsafe {
        match &previous_home {
            Some(value) => std::env::set_var("HOME", value),
            None => std::env::remove_var("HOME"),
        }
    }
}

#[test]
#[serial_test::serial]
fn a_missing_token_is_generated_and_saved() {
    with_temp_home(|home| {
        let token = token_for("claude-code").unwrap();
        let path = home
            .join(".warp")
            .join("agent-ops")
            .join("agent-tokens")
            .join("claude-code.token");
        let saved = std::fs::read_to_string(&path).unwrap();
        assert_eq!(saved, token.secret());
    });
}

#[test]
#[serial_test::serial]
fn calling_it_twice_returns_the_same_token() {
    with_temp_home(|_home| {
        let first = token_for("claude-code").unwrap();
        let second = token_for("claude-code").unwrap();
        assert_eq!(first, second);
    });
}

#[test]
#[serial_test::serial]
fn different_names_get_different_tokens() {
    with_temp_home(|_home| {
        let claude = token_for("claude-code").unwrap();
        let codex = token_for("codex").unwrap();
        assert_ne!(claude, codex);
    });
}

#[cfg(unix)]
#[test]
#[serial_test::serial]
fn the_token_file_is_saved_with_0600_permissions() {
    use std::os::unix::fs::PermissionsExt as _;

    with_temp_home(|home| {
        token_for("claude-code").unwrap();
        let path = home
            .join(".warp")
            .join("agent-ops")
            .join("agent-tokens")
            .join("claude-code.token");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    });
}

#[cfg(unix)]
#[test]
#[serial_test::serial]
fn a_token_file_readable_by_the_group_is_refused() {
    use std::os::unix::fs::PermissionsExt as _;

    with_temp_home(|home| {
        let dir = home.join(".warp").join("agent-ops").join("agent-tokens");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("claude-code.token");
        std::fs::write(&path, AgentToken::generate().secret()).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();

        match token_for("claude-code").unwrap_err() {
            PairingTokenError::Permissions { mode, .. } => assert_eq!(mode, 0o640),
            other => panic!("expected Permissions, got {other:?}"),
        }
    });
}

#[test]
#[serial_test::serial]
fn a_corrupt_token_file_is_rejected_rather_than_silently_replaced() {
    with_temp_home(|home| {
        let dir = home.join(".warp").join("agent-ops").join("agent-tokens");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("claude-code.token");
        std::fs::write(&path, "not-a-valid-token").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }

        assert!(matches!(
            token_for("claude-code").unwrap_err(),
            PairingTokenError::Invalid { .. }
        ));
    });
}
