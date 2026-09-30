use local_control::protocol::{RemoteAttachment, RemoteStream};

use super::*;

fn session() -> RemoteSessionRef {
    RemoteSessionRef {
        session_id: "Pane 7".to_owned(),
        host: "prod-1".to_owned(),
        user: "root".to_owned(),
    }
}

fn numbered(lines: &[(usize, &str)]) -> String {
    lines
        .iter()
        .map(|(number, line)| format!("{number:>6}\t{line}\n"))
        .collect()
}

#[test]
fn page_numbers_lines_from_the_offset() {
    let page = page("a\nb\nc\nd\n", 2, 2);
    assert_eq!(page.text, numbered(&[(2, "b"), (3, "c")]));
    assert_eq!(page.last_line, 3);
    assert_eq!(page.total_lines, 4);
}

#[test]
fn page_counts_a_last_line_without_newline() {
    let page = page("a\nb", 1, 10);
    assert_eq!(page.text, numbered(&[(1, "a"), (2, "b")]));
    assert_eq!(page.total_lines, 2);
    assert_eq!(super::page("", 1, 10).total_lines, 0);
    assert_eq!(super::page("\n", 1, 10).total_lines, 1);
}

#[test]
fn page_cuts_long_lines_and_stops_before_the_size_limit() {
    let long = "x".repeat(MAX_LINE_CHARS + 5);
    let page = page(&long, 1, 1);
    assert_eq!(
        page.text,
        format!("     1\t{}… [line cut]\n", "x".repeat(MAX_LINE_CHARS))
    );

    let content = format!("{}\n", "y".repeat(1000)).repeat(200);
    let page = super::page(&content, 1, 2000);
    assert!(page.text.len() <= MAX_PAGE_BYTES);
    assert!(page.last_line < 200);
}

#[test]
fn render_read_says_how_to_read_on() {
    let text = render_read(&session(), "/etc/a", "1\n2\n3\n", 1, 2);
    assert_eq!(
        text,
        format!(
            "root@prod-1:/etc/a (3 lines, 6 bytes)\n{}\
             [showing lines 1-2 of 3; call read_file with offset=3 to read on]\n",
            numbered(&[(1, "1"), (2, "2")])
        )
    );
}

#[test]
fn render_read_handles_empty_files_and_offsets_past_the_end() {
    assert_eq!(
        render_read(&session(), "/e", "", 1, 10),
        "root@prod-1:/e (0 lines, 0 bytes)\n(empty file)\n"
    );
    assert_eq!(
        render_read(&session(), "/e", "a\n", 5, 10),
        "root@prod-1:/e (1 lines, 2 bytes)\n(offset 5 is past the end of the file)\n"
    );
}

#[test]
fn edit_snippet_shows_three_lines_around_the_change() {
    let content: String = (1..=20).map(|n| format!("line {n}\n")).collect();
    let first_change = content.find("line 10").expect("line 10 exists");

    let snippet = edit_snippet(&content, first_change, "line 10");

    let expected: Vec<(usize, String)> = (7..=13).map(|n| (n, format!("line {n}"))).collect();
    let expected: Vec<(usize, &str)> = expected.iter().map(|(n, s)| (*n, s.as_str())).collect();
    assert_eq!(snippet, numbered(&expected));
}

#[test]
fn edit_snippet_at_the_top_starts_at_line_one() {
    let snippet = edit_snippet("a\nb\nc\nd\ne\nf\n", 0, "a\nb");
    assert_eq!(
        snippet,
        numbered(&[(1, "a"), (2, "b"), (3, "c"), (4, "d"), (5, "e")])
    );
}

fn stream(text: &str, total_bytes: u64, truncated: bool) -> RemoteStream {
    RemoteStream {
        text: text.to_owned(),
        total_bytes,
        truncated,
    }
}

fn exec_result(stdout: RemoteStream, stderr: RemoteStream) -> RemoteExecResult {
    RemoteExecResult {
        session: session(),
        cwd: Some("/etc".to_owned()),
        exit_code: 1,
        timed_out: false,
        duration_ms: 1234,
        stdout,
        stderr,
    }
}

#[test]
fn render_exec_labels_the_streams() {
    let result = exec_result(stream("out\n", 4, false), stream("err", 3, false));
    assert_eq!(
        render_exec(&result),
        "exit_code: 1 (1.2s) root@prod-1:/etc\n--- stdout ---\nout\n--- stderr ---\nerr\n"
    );
}

#[test]
fn render_exec_notes_cut_output_timeouts_and_silence() {
    let mut result = exec_result(
        stream("head…tail\n", 3 * 1024 * 1024, true),
        stream("", 0, false),
    );
    result.timed_out = true;
    result.exit_code = 124;
    result.duration_ms = 5;
    assert_eq!(
        render_exec(&result),
        "exit_code: 124 (5ms) root@prod-1:/etc\n\
         [the command timed out and was stopped]\n\
         --- stdout ---\nhead…tail\n\
         [stdout was cut: the command printed 3.0 MiB; filter it with grep, head or tail]\n"
    );

    let silent = exec_result(stream("", 0, false), stream("", 0, false));
    assert!(render_exec(&silent).ends_with("(no output)\n"));
}

fn summary(
    session_id: &str,
    session_type: RemoteSessionKind,
    attached: Option<RemoteAttachment>,
) -> RemoteSessionSummary {
    RemoteSessionSummary {
        session_id: session_id.to_owned(),
        window_index: 0,
        tab_index: 0,
        pane_index: 0,
        is_active: session_id == "Pane 1",
        session_type,
        host: "prod-1".to_owned(),
        user: "root".to_owned(),
        shell: "bash".to_owned(),
        cwd: Some("/root".to_owned()),
        attached,
    }
}

#[test]
fn render_sessions_quotes_ids_and_says_what_is_usable() {
    let text = render_sessions(&[
        summary(
            "Pane 1",
            RemoteSessionKind::Remote,
            Some(RemoteAttachment {
                access: RemoteAccess::Full,
                idle_secs: 0,
                expires_in_secs: 1800,
                exec_count: 2,
            }),
        ),
        summary("Pane 2", RemoteSessionKind::Remote, None),
        summary("Pane 3", RemoteSessionKind::Local, None),
    ]);
    assert_eq!(
        text,
        "Warp terminal sessions (pass session_id exactly as shown):\n\
         - session_id \"Pane 1\" (focused): root@prod-1 cwd /root shell bash — attached: exec, read \
         and write; expires after 30 min idle\n\
         - session_id \"Pane 2\": root@prod-1 cwd /root shell bash — not attached (the user has to \
         allow agents in that pane first)\n\
         - session_id \"Pane 3\": root@prod-1 cwd /root shell bash — local session, not usable \
         (use your own shell)\n"
    );
    assert_eq!(
        render_sessions(&[]),
        "No terminal sessions are open in Warp."
    );
}

#[test]
fn human_bytes_picks_a_unit() {
    assert_eq!(human_bytes(1023), "1023 bytes");
    assert_eq!(human_bytes(1536), "1.5 KiB");
    assert_eq!(human_bytes(5 * 1024 * 1024), "5.0 MiB");
}

fn listed_host(alias: &str) -> RemoteHostSummary {
    RemoteHostSummary {
        alias: alias.to_owned(),
        tags: Vec::new(),
        source: RemoteHostSource::Warp,
        missing: false,
        connection: None,
        root_login: RemoteRootLogin::None,
        transport: RemoteTransport::Direct,
        sessions: Vec::new(),
        mirror: None,
    }
}

#[test]
fn render_hosts_says_when_the_list_is_cut() {
    let text = render_hosts(&RemoteHostListResult {
        hosts: vec![listed_host("a")],
        total: 454,
    });
    assert!(
        text.starts_with("Warp's server directory (1 of 454 shown"),
        "{text}"
    );
    assert!(
        text.contains(
            "- a: connection unknown — created in Warp; no root access noted; transport direct\n"
        ),
        "{text}"
    );
}

#[test]
fn render_hosts_handles_an_empty_list_and_a_gone_host() {
    assert_eq!(
        render_hosts(&RemoteHostListResult {
            hosts: Vec::new(),
            total: 0
        }),
        "No servers match in Warp's server directory."
    );
    let mut gone = listed_host("old");
    gone.missing = true;
    let text = render_hosts(&RemoteHostListResult {
        hosts: vec![gone],
        total: 1,
    });
    assert!(text.contains("no longer in ~/.ssh/config"), "{text}");
}

#[test]
fn render_hosts_shows_a_mirror_with_nothing_synced_and_a_cut_path_list() {
    let mut host = listed_host("a");
    host.mirror = Some(RemoteHostMirror {
        dir: "/m/a".to_owned(),
        synced_paths: Vec::new(),
        synced_paths_truncated: false,
    });
    let text = render_hosts(&RemoteHostListResult {
        hosts: vec![host.clone()],
        total: 1,
    });
    assert!(
        text.contains("mirror: /m/a\n    synced: nothing yet"),
        "{text}"
    );

    host.mirror = Some(RemoteHostMirror {
        dir: "/m/a".to_owned(),
        synced_paths: vec!["/etc".to_owned(), "/srv".to_owned()],
        synced_paths_truncated: true,
    });
    let text = render_hosts(&RemoteHostListResult {
        hosts: vec![host],
        total: 1,
    });
    assert!(
        text.contains("synced: /etc, /srv, … (more not shown)"),
        "{text}"
    );
}

fn open_result(
    status: RemoteOpenStatus,
    elevation: RemoteOpenElevation,
) -> RemoteSessionOpenResult {
    let ready = status == RemoteOpenStatus::Ready;
    RemoteSessionOpenResult {
        status,
        session_id: "7".to_owned(),
        host_alias: "lab-1".to_owned(),
        host: ready.then(|| "lab-1.internal".to_owned()),
        user: ready.then(|| "root".to_owned()),
        access: RemoteAccess::Full,
        elevation,
        note: None,
    }
}

#[test]
fn a_ready_session_is_named_with_who_it_is_signed_in_as() {
    let text = render_open(&open_result(
        RemoteOpenStatus::Ready,
        RemoteOpenElevation::Elevated,
    ));
    assert_eq!(
        text,
        "Opened session_id \"7\" on lab-1: root@lab-1.internal, full access. Root: yes, through \
         sudo -i."
    );
}

#[test]
fn a_session_that_is_not_ready_says_what_the_agent_is_waiting_for() {
    let mut result = open_result(
        RemoteOpenStatus::Connecting,
        RemoteOpenElevation::NotRequested,
    );
    result.note = Some("Wait for the user.".to_owned());
    let text = render_open(&result);
    assert!(text.starts_with("The session_id \"7\" on lab-1 is not ready yet"));
    assert!(text.ends_with("Wait for the user."));
    assert!(!text.contains("Root:"));
}

#[test]
fn closing_a_session_is_confirmed_by_id() {
    let closed = render_close(&RemoteSessionCloseResult {
        session_id: "7".to_owned(),
        closed: true,
    });
    assert_eq!(closed, "Closed session_id \"7\".");
}
