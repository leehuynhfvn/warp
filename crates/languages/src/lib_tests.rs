use std::path::Path;

use warp_util::standardized_path::StandardizedPath;

use crate::{
    SUPPORTED_LANGUAGES, detect_language_name, language_by_filename, language_by_local_filename,
    language_by_name, load_language,
};

/// Validate that every supported language can be loaded successfully.
/// This catches invalid node types, syntax errors, and other issues in .scm query files
/// (highlights, indents, identifiers) that would otherwise only surface at runtime.
#[test]
fn all_supported_languages_load_successfully() {
    let failures: Vec<_> = SUPPORTED_LANGUAGES
        .iter()
        .filter(|lang| load_language(lang).is_none())
        .collect();

    assert!(
        failures.is_empty(),
        "The following languages failed to load:\n{}",
        failures
            .iter()
            .map(|lang| format!("  - {lang}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// Both `.html` and the legacy three-character `.htm` extension should resolve to
/// the same HTML language entry. `.htm` is widely produced by static-site generators
/// and historical web tooling (DOS 8.3 filename limits) and is already treated as
/// an HTML/text file elsewhere in the codebase
/// (see `is_development_text_extension` in `crates/warp_util/src/file_type.rs`).
#[test]
fn html_extensions_resolve_to_html() {
    for filename in ["index.html", "index.htm"] {
        let path = StandardizedPath::try_new(&format!("/tmp/{filename}"))
            .expect("test path should be absolute");
        let language = language_by_filename(&path)
            .unwrap_or_else(|| panic!("expected {filename} to resolve to a language"));
        assert_eq!(
            language.display_name(),
            "HTML",
            "{filename} should resolve to HTML",
        );
    }
}

#[test]
fn local_html_extensions_resolve_to_html() {
    for filename in ["index.html", "index.htm"] {
        let path = Path::new(filename);
        let language = language_by_local_filename(path)
            .unwrap_or_else(|| panic!("expected {filename} to resolve to a language"));
        assert_eq!(
            language.display_name(),
            "HTML",
            "{filename} should resolve to HTML",
        );
    }
}

/// `.command` is the macOS convention for double-clickable shell scripts.
/// Make sure `language_by_filename` recognizes it as shell so the editor
/// renders syntax highlighting instead of the
/// "Language support is unavailable for this file type" footer.
#[test]
fn command_extension_resolves_to_shell() {
    let path =
        StandardizedPath::try_new("/tmp/script.command").expect("test path should be absolute");
    let language =
        language_by_filename(&path).expect("`.command` files should resolve to a language");
    assert_eq!(language.display_name(), "Shell");
}

#[test]
fn local_command_extension_resolves_to_shell() {
    let language = language_by_local_filename(Path::new("script.command"))
        .expect("`.command` files should resolve to a language");
    assert_eq!(language.display_name(), "Shell");
}

/// `.md` and `.markdown` should resolve to the Markdown language so the editor applies
/// syntax highlighting to Markdown source files.
#[test]
fn markdown_extensions_resolve_to_markdown() {
    for filename in ["README.md", "notes.markdown"] {
        let path = StandardizedPath::try_new(&format!("/tmp/{filename}"))
            .expect("test path should be absolute");
        let language = language_by_filename(&path)
            .unwrap_or_else(|| panic!("expected {filename} to resolve to a language"));
        assert_eq!(
            language.display_name(),
            "Markdown",
            "{filename} should resolve to Markdown",
        );
    }
}

#[test]
fn server_configuration_files_are_recognized_by_name_folder_and_extension() {
    let cases = [
        ("/etc/nginx/nginx.conf", Some("nginx")),
        ("/etc/nginx/conf.d/site.conf", Some("nginx")),
        ("/etc/nginx/sites-available/default", Some("nginx")),
        (
            "/home/me/.warp/mirrors/web-1/etc/nginx/conf.d/a.conf",
            Some("nginx"),
        ),
        ("/etc/ssh/sshd_config", Some("ssh-config")),
        (
            "/etc/ssh/sshd_config.d/50-cloud-init.conf",
            Some("ssh-config"),
        ),
        ("/etc/systemd/system/app.service", Some("ini")),
        ("/etc/systemd/journald.conf", Some("ini")),
        ("/etc/mysql/my.cnf", Some("ini")),
        ("/etc/php/8.2/fpm/php.ini", Some("ini")),
        ("/etc/caddy/Caddyfile", Some("caddy")),
        ("/srv/app/templates/site.conf.j2", Some("jinja2")),
        ("/tmp/fix.patch", Some("diff")),
        ("/root/.profile", Some("shell")),
        // `.conf` outside a known folder is ambiguous.
        ("/etc/haproxy/haproxy.cfg", None),
        ("/etc/apache2/apache2.conf", None),
        ("/etc/hosts", None),
    ];
    for (path, expected) in cases {
        assert_eq!(detect_language_name(path, None), expected, "{path}");
    }
}

#[test]
fn a_file_without_a_known_name_is_recognized_by_its_shebang() {
    let cases = [
        ("#!/bin/bash", Some("shell")),
        ("#!/bin/sh -e", Some("shell")),
        ("#!/usr/bin/env python3", Some("python")),
        ("#!/usr/bin/env -S python3.11 -u", Some("python")),
        ("#!/usr/bin/awk -f", Some("awk")),
        ("#!/usr/bin/env unknown-tool", None),
        ("# not a shebang", None),
    ];
    for (first_line, expected) in cases {
        assert_eq!(
            detect_language_name("/usr/local/bin/backup", Some(first_line)),
            expected,
            "{first_line}"
        );
    }
}

#[test]
fn a_known_name_wins_over_the_shebang() {
    assert_eq!(
        detect_language_name("/opt/run.py", Some("#!/bin/bash")),
        Some("python")
    );
}

#[test]
fn new_server_languages_parse_a_sample() {
    let samples = [
        ("nginx", "server {\n    listen 80;\n    # comment\n}\n"),
        ("ini", "[Unit]\nDescription=App\n"),
        ("ssh-config", "Port 22\nPermitRootLogin no\n"),
        ("diff", "--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n"),
        ("awk", "{ print $1 }\n"),
        ("jinja2", "{{ name }}\n"),
        ("caddy", "example.com {\n\treverse_proxy :8080\n}\n"),
    ];
    for (name, sample) in samples {
        let language = language_by_name(name).unwrap_or_else(|| panic!("{name} loads"));
        let mut parser = arborium::tree_sitter::Parser::new();
        parser.set_language(&language.grammar).unwrap();
        let tree = parser.parse(sample, None).unwrap();
        assert!(
            tree.root_node().child_count() > 0,
            "{name} produced an empty tree"
        );
    }
}
