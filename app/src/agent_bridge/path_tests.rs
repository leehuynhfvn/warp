use super::*;

fn normalize(input: &str) -> Result<String, AgentBridgeError> {
    normalize_path(input, Some("/etc/nginx"))
}

#[test]
fn absolute_paths_are_cleaned_up() {
    assert_eq!(normalize("/etc//nginx/./nginx.conf").unwrap(), "/etc/nginx/nginx.conf");
    assert_eq!(normalize("  /etc/hosts  ").unwrap(), "/etc/hosts");
    assert_eq!(normalize("/etc/hosts/").unwrap(), "/etc/hosts");
}

#[test]
fn relative_paths_are_taken_from_the_session_directory() {
    assert_eq!(normalize("conf.d/site.conf").unwrap(), "/etc/nginx/conf.d/site.conf");
    assert!(normalize_path("conf.d/site.conf", None).is_err());
    assert!(normalize_path("conf.d/site.conf", Some("relative")).is_err());
}

#[test]
fn unsafe_or_meaningless_paths_are_refused() {
    for input in [
        "",
        "   ",
        "~/.bashrc",
        "/etc/../shadow",
        "../shadow",
        "/etc/a\nb",
        "/etc/a\0b",
        "/",
        "/.",
        "/proc/meminfo",
        "/sys/kernel/x",
        "/dev/null",
        "/run/nginx.pid",
    ] {
        assert!(
            matches!(normalize(input), Err(AgentBridgeError::InvalidParams(_))),
            "{input:?} should be refused"
        );
    }
    assert!(normalize(&format!("/{}", "a".repeat(MAX_PATH_BYTES))).is_err());
}

#[test]
fn hidden_and_git_files_are_ordinary_files() {
    assert_eq!(normalize("/srv/app/.git/config").unwrap(), "/srv/app/.git/config");
    assert_eq!(normalize("/root/.bashrc").unwrap(), "/root/.bashrc");
}

#[test]
fn a_directory_that_merely_starts_like_a_pseudo_fs_is_allowed() {
    assert_eq!(normalize("/running/x").unwrap(), "/running/x");
    assert_eq!(normalize("/etc/proc/x").unwrap(), "/etc/proc/x");
}

#[test]
fn a_working_directory_must_be_absolute_and_clean() {
    assert!(validate_cwd("/srv/app").is_ok());
    for cwd in ["srv", "", "~", "/srv\napp", "/srv\0"] {
        assert!(validate_cwd(cwd).is_err(), "{cwd:?} should be refused");
    }
}
