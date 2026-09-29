use std::iter;

use warp_util::path::CleanPathResult;

use super::super::TerminalView;
use super::{GridHighlightedLink, path_without_trailing_sentence_punctuation};
use crate::terminal::model::grid::grid_handler::PossiblePath;
use crate::terminal::model::index::Point;
use crate::terminal::model::session::SessionId;
use crate::terminal::model::terminal_model::WithinModel;

#[test]
fn strips_only_sentence_periods() {
    // A trailing period after a real file name is sentence punctuation.
    assert_eq!(
        path_without_trailing_sentence_punctuation("notes/README.md.").map(|trimmed| trimmed.path),
        Some("notes/README.md")
    );
    assert_eq!(
        path_without_trailing_sentence_punctuation(".gitignore.").map(|trimmed| trimmed.path),
        Some(".gitignore")
    );
    assert_eq!(
        path_without_trailing_sentence_punctuation("C:/Users/c/warp-md-test.md.")
            .map(|trimmed| trimmed.path),
        Some("C:/Users/c/warp-md-test.md")
    );

    // No trailing period -> nothing to trim.
    assert_eq!(
        path_without_trailing_sentence_punctuation("notes/README.md").map(|trimmed| trimmed.path),
        None
    );

    // `.`/`..` path components must be preserved, not treated as punctuation.
    assert_eq!(
        path_without_trailing_sentence_punctuation(".").map(|trimmed| trimmed.path),
        None
    );
    assert_eq!(
        path_without_trailing_sentence_punctuation("..").map(|trimmed| trimmed.path),
        None
    );
    assert_eq!(
        path_without_trailing_sentence_punctuation("foo/.").map(|trimmed| trimmed.path),
        None
    );
    assert_eq!(
        path_without_trailing_sentence_punctuation("foo/..").map(|trimmed| trimmed.path),
        None
    );
    assert_eq!(
        path_without_trailing_sentence_punctuation("foo..").map(|trimmed| trimmed.path),
        None
    );
}

#[test]
fn strips_trailing_fullwidth_sentence_punctuation() {
    let trimmed = path_without_trailing_sentence_punctuation("notes/README.md，")
        .expect("fullwidth comma should be stripped");
    assert_eq!(trimmed.path, "notes/README.md");
    assert_eq!(trimmed.removed_width, 2);

    let trimmed = path_without_trailing_sentence_punctuation("notes/README.md。！？")
        .expect("CJK sentence punctuation should be stripped");
    assert_eq!(trimmed.path, "notes/README.md");
    assert_eq!(trimmed.removed_width, 6);
}

// Regression test for https://github.com/warpdotdev/warp/issues/11477:
// a `.md` path at the end of a sentence captured the trailing period, so the
// resolved file and the highlight range ended in `.md.` and the file failed
// markdown classification. The trailing period must be excluded from both.
#[test]
fn compute_valid_paths_excludes_trailing_sentence_period() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("warp-md-test.md");
    std::fs::write(&file, "# Hello\n").unwrap();

    // The captured token as it would appear in `Drafted at <abs path>.`
    let token = format!("{}.", file.to_string_lossy());
    let end_col = token.chars().count() - 1;
    let candidate = WithinModel::AltScreen(PossiblePath {
        path: CleanPathResult {
            path: token,
            line_and_column_num: None,
        },
        range: Point { row: 0, col: 0 }..=Point {
            row: 0,
            col: end_col,
        },
    });

    let link = TerminalView::compute_valid_paths(
        dir.path().to_str().unwrap(),
        iter::once(candidate),
        1000,
        None,
    )
    .expect("the markdown file should be detected as a link");

    let GridHighlightedLink::File(file_link) = link else {
        panic!("expected a file link");
    };
    let file_link = file_link.get_inner();

    // The resolved file excludes the trailing period (so it classifies as `.md`)...
    assert_eq!(
        file_link.absolute_path.file_name().unwrap(),
        "warp-md-test.md"
    );
    // ...and the highlighted range stops before the trailing period.
    assert_eq!(
        *file_link.link.range().end(),
        Point {
            row: 0,
            col: end_col - 1,
        }
    );
}

#[test]
fn compute_valid_paths_excludes_trailing_fullwidth_sentence_punctuation() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("warp-md-test.md");
    std::fs::write(&file, "# Hello\n").unwrap();

    let token = format!("{}，", file.to_string_lossy());
    let punctuation_width = 2;
    let end_col = token.chars().count();
    let candidate = WithinModel::AltScreen(PossiblePath {
        path: CleanPathResult {
            path: token,
            line_and_column_num: None,
        },
        range: Point { row: 0, col: 0 }..=Point {
            row: 0,
            col: end_col,
        },
    });

    let link = TerminalView::compute_valid_paths(
        dir.path().to_str().unwrap(),
        iter::once(candidate),
        1000,
        None,
    )
    .expect("the markdown file should be detected as a link");

    let GridHighlightedLink::File(file_link) = link else {
        panic!("expected a file link");
    };
    let file_link = file_link.get_inner();

    assert_eq!(
        file_link.absolute_path.file_name().unwrap(),
        "warp-md-test.md"
    );
    assert_eq!(
        *file_link.link.range().end(),
        Point {
            row: 0,
            col: end_col - punctuation_width,
        }
    );
}

#[test]
fn compute_valid_paths_keeps_trailing_fullwidth_punctuation_when_it_is_the_filename() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("warp-md-test.md，");
    std::fs::write(&file, "# Hello\n").unwrap();

    let token = file.to_string_lossy().to_string();
    let end_col = token.chars().count();
    let candidate = WithinModel::AltScreen(PossiblePath {
        path: CleanPathResult {
            path: token,
            line_and_column_num: None,
        },
        range: Point { row: 0, col: 0 }..=Point {
            row: 0,
            col: end_col,
        },
    });

    let link = TerminalView::compute_valid_paths(
        dir.path().to_str().unwrap(),
        iter::once(candidate),
        1000,
        None,
    )
    .expect("the file with fullwidth punctuation should be detected as a link");

    let GridHighlightedLink::File(file_link) = link else {
        panic!("expected a file link");
    };
    let file_link = file_link.get_inner();

    assert_eq!(
        file_link.absolute_path.file_name().unwrap(),
        "warp-md-test.md，"
    );
    assert_eq!(
        *file_link.link.range().end(),
        Point {
            row: 0,
            col: end_col,
        }
    );
}

/// Candidates for a hover at `hover_col` of `line`, the way the grid builds them: every span of
/// whole fragments that covers the hover point, with any `:line[:col]` suffix parsed out.
fn candidates_around(line: &str, hover_col: usize) -> Vec<WithinModel<PossiblePath>> {
    let is_separator =
        |c: char| crate::terminal::model::grid::grid_handler::is_file_link_separator(c) || c == ' ';
    let mut bounds = vec![0];
    for (index, c) in line.char_indices() {
        if is_separator(c) {
            bounds.extend([index, index + c.len_utf8()]);
        }
    }
    bounds.push(line.len());
    bounds.dedup();
    let mut candidates = Vec::new();
    for &start in bounds.iter().filter(|start| **start <= hover_col) {
        for &end in bounds.iter().filter(|end| **end > hover_col) {
            candidates.push(WithinModel::AltScreen(PossiblePath {
                path: CleanPathResult::with_line_and_column_number(&line[start..end]),
                range: Point { row: 0, col: start }..=Point {
                    row: 0,
                    col: end - 1,
                },
            }));
        }
    }
    candidates
}

fn remote_link(line: &str, word: &str, pwd: &str) -> Option<(String, Option<usize>, usize, usize)> {
    let hover_col = line.find(word).expect("word is in the line") + 1;
    TerminalView::remote_link_from_candidates(
        candidates_around(line, hover_col).into_iter(),
        pwd,
        SessionId::from(1),
        1000,
    )
    .map(|link| {
        let link = link.get_inner();
        (
            link.remote_path.clone(),
            link.line_and_column_num.map(|line| line.line_num),
            link.link.range.start().col,
            link.link.range.end().col,
        )
    })
}

#[test]
fn a_file_name_in_ls_output_links_to_the_file_in_the_blocks_directory() {
    let line = "-rw-r--r-- 1 root root 1447 Sep 29 nginx.conf";
    let start = line.find("nginx.conf").unwrap();

    assert_eq!(
        remote_link(line, "nginx.conf", "/etc/nginx"),
        Some((
            "/etc/nginx/nginx.conf".to_owned(),
            None,
            start,
            line.len() - 1
        ))
    );
    assert_eq!(remote_link(line, "root", "/etc/nginx"), None);
    assert_eq!(remote_link(line, "1447", "/etc/nginx"), None);
}

#[test]
fn a_path_with_a_line_number_in_an_error_links_to_that_line() {
    let line = "nginx: [emerg] unknown directive \"foo\" in /etc/nginx/conf.d/a.conf:12";

    let (path, line_num, _, end) = remote_link(line, "/etc/nginx", "/root").unwrap();

    assert_eq!(path, "/etc/nginx/conf.d/a.conf");
    assert_eq!(line_num, Some(12));
    assert_eq!(end, line.len() - 1);
}

#[test]
fn quotes_commas_and_a_final_period_are_left_out_of_the_link() {
    let line = "edit \"/etc/hosts\", then /etc/fstab.";

    let (path, _, start, end) = remote_link(line, "/etc/hosts", "/").unwrap();
    assert_eq!(path, "/etc/hosts");
    assert_eq!((start, end), (6, 15));

    let (path, _, _, end) = remote_link(line, "/etc/fstab", "/").unwrap();
    assert_eq!(path, "/etc/fstab");
    assert_eq!(end, line.len() - 2);
}
