use super::*;

#[test]
fn an_attach_message_names_the_session_the_access_and_how_to_undo_it() {
    let full = attached_message(Access::Full, "root", "prod-1");
    assert!(full.contains("run commands and edit files as root@prod-1"), "{full}");
    assert!(full.contains("30 minutes"), "{full}");
    assert!(full.contains("Agent Bridge: Revoke access to this session"), "{full}");

    let read_only = attached_message(Access::ReadOnly, "root", "prod-1");
    assert!(read_only.contains("read files (read-only)"), "{read_only}");
    assert!(!read_only.contains("run commands"), "{read_only}");
}

#[test]
fn the_messages_do_not_name_a_particular_agent() {
    let texts = [
        attached_message(Access::Full, "root", "h"),
        attached_message(Access::ReadOnly, "root", "h"),
        revoked_message(true).to_owned(),
        revoked_message(false).to_owned(),
        revoked_all_message(2),
    ];
    for text in texts {
        assert!(!text.to_lowercase().contains("claude"), "{text}");
    }
}

#[test]
fn revoking_all_counts_sessions() {
    assert_eq!(revoked_all_message(0), "No session had agent access.");
    assert_eq!(revoked_all_message(1), "Revoked agent access to 1 session.");
    assert_eq!(revoked_all_message(3), "Revoked agent access to 3 sessions.");
}

#[test]
fn the_setup_command_quotes_the_executable_path() {
    assert_eq!(
        setup_command(Path::new("/projects/warp/target/debug/warp")),
        "claude mcp add --scope user warp-bridge -- '/projects/warp/target/debug/warp' --warpctrl mcp"
    );
    let tricky = setup_command(Path::new("/opt/My Apps/it's/warp"));
    assert!(tricky.contains(r"'/opt/My Apps/it'\''s/warp'"), "{tricky}");
}
