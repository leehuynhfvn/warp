use super::*;

#[test]
fn identical_files_have_no_diff() {
    assert!(single_file_diff("a\nb\n", "a\nb\n", MAX_DIFF_LINES).is_empty());
}

#[test]
fn a_changed_line_shows_with_its_context() {
    let old = "user www-data;\nworker_processes 1;\n# comment\npid /run/nginx.pid;\n";
    let new = "user www-data;\nworker_processes 4;\n# comment\npid /run/nginx.pid;\n";

    assert_eq!(
        single_file_diff(old, new, MAX_DIFF_LINES),
        [
            "@@ -1,4 +1,4 @@",
            " user www-data;",
            "-worker_processes 1;",
            "+worker_processes 4;",
            " # comment",
            " pid /run/nginx.pid;",
        ]
    );
}

#[test]
fn a_long_diff_is_cut_with_a_count_of_what_was_left_out() {
    let old: String = (0..30).map(|i| format!("old {i}\n")).collect();
    let new: String = (0..30).map(|i| format!("new {i}\n")).collect();

    let lines = single_file_diff(&old, &new, 10);

    assert_eq!(lines.len(), 11);
    assert_eq!(lines[10], "… 51 more lines");
}

#[test]
fn windows_line_endings_do_not_leak_into_the_lines() {
    let lines = single_file_diff("a\r\n", "b\r\n", MAX_DIFF_LINES);

    assert_eq!(lines, ["@@ -1 +1 @@", "-a", "+b"]);
}
