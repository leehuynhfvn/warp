//! Text of the tool results, shaped like the output of Claude Code's own tools.
use local_control::protocol::{
    RemoteAccess, RemoteExecResult, RemoteExecVisibleResult, RemoteSessionKind, RemoteSessionRef,
    RemoteSessionSummary, RemoteStream,
};

/// Longer lines are cut, as Claude Code's Read tool does.
const MAX_LINE_CHARS: usize = 2000;

/// The numbered lines of one `read_file` result stay under this.
const MAX_PAGE_BYTES: usize = 80 * 1024;

/// Lines shown before and after an edit.
const SNIPPET_CONTEXT_LINES: usize = 3;

/// Numbered lines of a file, `cat -n` style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Page {
    pub text: String,
    /// Number of the last line in `text`; `0` when it has none.
    pub last_line: usize,
    pub total_lines: usize,
}

/// Up to `limit` lines of `content` starting at line `first_line` (from 1).
pub(super) fn page(content: &str, first_line: usize, limit: usize) -> Page {
    let lines = split_lines(content);
    let first_line = first_line.max(1);
    let mut text = String::new();
    let mut last_line = 0;
    for (index, line) in lines.iter().enumerate().skip(first_line - 1).take(limit) {
        let number = index + 1;
        let numbered = format!("{number:>6}\t{}\n", cut_line(line));
        if !text.is_empty() && text.len() + numbered.len() > MAX_PAGE_BYTES {
            break;
        }
        text.push_str(&numbered);
        last_line = number;
    }
    Page {
        text,
        last_line,
        total_lines: lines.len(),
    }
}

/// The lines of `content`, without the empty one after a final newline.
fn split_lines(content: &str) -> Vec<&str> {
    if content.is_empty() {
        return Vec::new();
    }
    let content = content.strip_suffix('\n').unwrap_or(content);
    content.split('\n').collect()
}

fn cut_line(line: &str) -> String {
    match line.char_indices().nth(MAX_LINE_CHARS) {
        Some((end, _)) => format!("{}… [line cut]", &line[..end]),
        None => line.to_owned(),
    }
}

/// `read_file` result: a header, the numbered lines and how to read on.
pub(super) fn render_read(
    session: &RemoteSessionRef,
    path: &str,
    content: &str,
    first_line: usize,
    limit: usize,
) -> String {
    let page = page(content, first_line, limit);
    let mut text = format!(
        "{} ({} lines, {})\n",
        place(session, path),
        page.total_lines,
        human_bytes(content.len() as u64)
    );
    if page.total_lines == 0 {
        text.push_str("(empty file)\n");
        return text;
    }
    if page.last_line == 0 {
        text.push_str(&format!(
            "(offset {first_line} is past the end of the file)\n"
        ));
        return text;
    }
    text.push_str(&page.text);
    if page.last_line < page.total_lines {
        text.push_str(&format!(
            "[showing lines {first_line}-{} of {}; call read_file with offset={} to read on]\n",
            page.last_line,
            page.total_lines,
            page.last_line + 1
        ));
    }
    text
}

/// The lines around an edit that starts at byte `first_change` of `content` and inserted
/// `new_text`.
pub(super) fn edit_snippet(content: &str, first_change: usize, new_text: &str) -> String {
    let start_line = content
        .get(..first_change)
        .map_or(1, |before| before.matches('\n').count() + 1);
    let changed_lines = new_text.matches('\n').count() + 1;
    let first_line = start_line.saturating_sub(SNIPPET_CONTEXT_LINES).max(1);
    let limit = start_line - first_line + changed_lines + SNIPPET_CONTEXT_LINES;
    page(content, first_line, limit).text
}

/// `exec` result: status line, then the non-empty streams.
pub(super) fn render_exec(result: &RemoteExecResult) -> String {
    let cwd = result.cwd.as_deref().unwrap_or("");
    let mut text = format!(
        "exit_code: {} ({}) {}\n",
        result.exit_code,
        human_duration(result.duration_ms),
        place(&result.session, cwd)
    );
    if result.timed_out {
        text.push_str("[the command timed out and was stopped]\n");
    }
    let streams = [("stdout", &result.stdout), ("stderr", &result.stderr)];
    if streams.iter().all(|(_, stream)| stream.text.is_empty()) {
        text.push_str("(no output)\n");
        return text;
    }
    for (name, stream) in streams {
        push_stream(&mut text, name, stream);
    }
    text
}

/// `exec_visible` result.
pub(super) fn render_exec_visible(result: &RemoteExecVisibleResult) -> String {
    let cwd = result.cwd.as_deref().unwrap_or("");
    let duration = human_duration(result.duration_ms);
    let place = place(&result.session, cwd);
    let mut text = match result.exit_code {
        Some(exit_code) => format!("exit_code: {exit_code} ({duration}) {place}\n"),
        None => {
            let waiting_program = if result.alt_screen {
                " in a full-screen program (pager or editor) that waits for the user"
            } else {
                ""
            };
            format!(
                "still running after {duration} {place}\n[The command keeps running in the \
                 user's terminal{waiting_program}. Tell the user, and read its result later with \
                 recent_output.]\n"
            )
        }
    };
    if result.output.is_empty() {
        text.push_str("(no output)\n");
        return text;
    }
    let heading = if result.still_running {
        "output so far"
    } else {
        "output"
    };
    text.push_str(&format!("--- {heading} ---\n{}", result.output));
    if !result.output.ends_with('\n') {
        text.push('\n');
    }
    if result.truncated {
        text.push_str(&format!(
            "[output was cut: the whole output is {} terminal rows; filter it with grep, head or \
             tail]\n",
            result.output_rows
        ));
    }
    text
}

fn push_stream(text: &mut String, name: &str, stream: &RemoteStream) {
    if stream.text.is_empty() {
        return;
    }
    text.push_str(&format!("--- {name} ---\n{}", stream.text));
    if !stream.text.ends_with('\n') {
        text.push('\n');
    }
    if stream.truncated {
        text.push_str(&format!(
            "[{name} was cut: the command printed {}; filter it with grep, head or tail]\n",
            human_bytes(stream.total_bytes)
        ));
    }
}

/// `list_sessions` result.
pub(super) fn render_sessions(sessions: &[RemoteSessionSummary]) -> String {
    if sessions.is_empty() {
        return "No terminal sessions are open in Warp.".to_owned();
    }
    let mut text = "Warp terminal sessions (pass session_id exactly as shown):\n".to_owned();
    for session in sessions {
        text.push_str(&render_session(session));
        text.push('\n');
    }
    text
}

fn render_session(session: &RemoteSessionSummary) -> String {
    let id = serde_json::Value::String(session.session_id.clone());
    let focus = if session.is_active { " (focused)" } else { "" };
    let status = match (&session.session_type, &session.attached) {
        (RemoteSessionKind::Local, _) => {
            "local session, not usable (use your own shell)".to_owned()
        }
        (RemoteSessionKind::Remote, None) => {
            "not attached (the user has to allow agents in that pane first)".to_owned()
        }
        (RemoteSessionKind::Remote, Some(attached)) => {
            let access = match attached.access {
                RemoteAccess::Full => "attached: exec, read and write",
                RemoteAccess::ReadOnly => "attached read-only: read_file and recent_output only",
            };
            format!(
                "{access}; expires after {} idle",
                human_secs(attached.expires_in_secs)
            )
        }
    };
    let cwd = session.cwd.as_deref().unwrap_or("?");
    format!(
        "- session_id {id}{focus}: {}@{} cwd {cwd} shell {} — {status}",
        session.user, session.host, session.shell
    )
}

/// `user@host:path`, which tells the model where something happened.
pub(super) fn place(session: &RemoteSessionRef, path: &str) -> String {
    format!("{}@{}:{path}", session.user, session.host)
}

pub(super) fn human_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * KIB;
    match bytes {
        0..KIB => format!("{bytes} bytes"),
        KIB..MIB => format!("{:.1} KiB", bytes as f64 / KIB as f64),
        _ => format!("{:.1} MiB", bytes as f64 / MIB as f64),
    }
}

fn human_duration(ms: u64) -> String {
    match ms {
        0..1000 => format!("{ms}ms"),
        _ => format!("{:.1}s", ms as f64 / 1000.0),
    }
}

fn human_secs(secs: u64) -> String {
    match secs {
        0..60 => format!("{secs}s"),
        _ => format!("{} min", secs / 60),
    }
}

#[cfg(test)]
#[path = "format_tests.rs"]
mod tests;
