use super::*;

#[test]
fn replaces_the_single_occurrence() {
    let edit =
        apply_edit("a\nworker_connections 768;\nb\n", "768", "2048", false).expect("edit applies");

    assert_eq!(edit.content, "a\nworker_connections 2048;\nb\n");
    assert_eq!(edit.replacements, 1);
    assert_eq!(edit.first_change, "a\nworker_connections ".len());
}

#[test]
fn several_occurrences_need_replace_all() {
    assert_eq!(
        apply_edit("x x x", "x", "y", false),
        Err(EditError::Ambiguous { count: 3 })
    );

    let edit = apply_edit("x x x", "x", "y", true).expect("edit applies");
    assert_eq!(edit.content, "y y y");
    assert_eq!(edit.replacements, 3);
    assert_eq!(edit.first_change, 0);
}

#[test]
fn a_missing_old_string_is_not_found() {
    assert_eq!(apply_edit("abc", "d", "e", true), Err(EditError::NotFound));
}

#[test]
fn empty_or_unchanged_edits_are_rejected() {
    assert_eq!(
        apply_edit("abc", "", "e", false),
        Err(EditError::EmptyOldString)
    );
    assert_eq!(
        apply_edit("abc", "b", "b", false),
        Err(EditError::Unchanged)
    );
}

#[test]
fn replacing_with_nothing_deletes() {
    let edit = apply_edit("keep\ndrop\nkeep\n", "drop\n", "", false).expect("edit applies");
    assert_eq!(edit.content, "keep\nkeep\n");
}

#[test]
fn multibyte_text_keeps_byte_offsets() {
    let edit = apply_edit("héllo wörld", "wörld", "world", false).expect("edit applies");
    assert_eq!(edit.content, "héllo world");
    assert_eq!(edit.first_change, "héllo ".len());
}
