use super::*;

#[test]
fn a_limit_within_range_is_accepted() {
    assert_eq!(parse_limit("1", 16), Some(1));
    assert_eq!(parse_limit(" 16 ", 16), Some(16));
    assert_eq!(parse_limit("8", 16), Some(8));
}

#[test]
fn a_limit_outside_the_range_is_rejected() {
    assert_eq!(parse_limit("0", 16), None);
    assert_eq!(parse_limit("17", 16), None);
}

#[test]
fn text_that_is_not_a_whole_number_is_rejected() {
    for input in ["", "  ", "abc", "-4", "4.5", "4 MiB", "99999999999999"] {
        assert_eq!(parse_limit(input, 16), None, "{input:?}");
    }
}
