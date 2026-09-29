use super::*;

const SAMPLE: &str = "\
user ops
hostname 203.0.113.9
port 2222
addressfamily any
identityfile ~/.ssh/id_ed25519
identityfile ~/.ssh/id_rsa
proxyjump gw@jump.example:22
";

#[test]
fn parses_the_details_ssh_reports() {
    let resolved = parse_ssh_g(SAMPLE).unwrap();
    assert_eq!(resolved.user, "ops");
    assert_eq!(resolved.hostname, "203.0.113.9");
    assert_eq!(resolved.port, 2222);
    assert_eq!(resolved.proxy_jump.as_deref(), Some("gw@jump.example:22"));
    assert_eq!(
        resolved.identity_files,
        ["~/.ssh/id_ed25519", "~/.ssh/id_rsa"]
    );
}

#[test]
fn no_jump_host_is_none() {
    let resolved = parse_ssh_g("user a\nhostname h\nport 22\nproxyjump none\n").unwrap();
    assert_eq!(resolved.proxy_jump, None);
    let resolved = parse_ssh_g("user a\nhostname h\nport 22\n").unwrap();
    assert_eq!(resolved.proxy_jump, None);
}

#[test]
fn a_missing_field_is_an_error() {
    assert!(parse_ssh_g("hostname h\nport 22\n").is_err());
    assert!(parse_ssh_g("user a\nport 22\n").is_err());
    assert!(parse_ssh_g("user a\nhostname h\n").is_err());
    assert!(parse_ssh_g("").is_err());
}

#[test]
fn a_port_that_is_not_a_number_is_an_error() {
    assert!(parse_ssh_g("user a\nhostname h\nport http\n").is_err());
    assert!(parse_ssh_g("user a\nhostname h\nport 70000\n").is_err());
}

#[test]
fn unknown_keywords_and_lines_without_a_value_are_ignored() {
    let resolved = parse_ssh_g("bogus\nuser a\nhostname h\nport 22\nx y z\n").unwrap();
    assert_eq!(resolved.hostname, "h");
}

#[test]
fn shows_user_host_and_a_port_that_is_not_the_default() {
    let mut resolved = parse_ssh_g("user ops\nhostname h\nport 22\n").unwrap();
    assert_eq!(resolved.display(), "ops@h");
    resolved.port = 2222;
    assert_eq!(resolved.display(), "ops@h:2222");
}

#[test]
fn an_unsafe_alias_never_reaches_ssh() {
    let error = SystemSsh.resolve(None, "-oProxyCommand=x").unwrap_err();
    assert!(error.contains("must start with"), "{error}");
}
