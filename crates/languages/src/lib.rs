use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use arborium::tree_sitter::{Language as ParserGrammar, Query};
use lazy_static::lazy_static;
use rust_embed::RustEmbed;
use serde::{Deserialize, Serialize};
use warp_editor::content::text::IndentUnit;
use warp_util::standardized_path::StandardizedPath;

#[derive(RustEmbed)]
#[folder = "grammars"]
struct Grammars;

lazy_static! {
    static ref LANGUAGE_REGISTRY: LanguageRegistry = LanguageRegistry::new();
}

pub const SUPPORTED_LANGUAGES: [&str; 42] = [
    "rust",
    "golang",
    "yaml",
    "python",
    "javascript",
    "jsx",
    "typescript",
    "tsx",
    "java",
    "cpp",
    "shell",
    "csharp",
    "html",
    "css",
    "c",
    "json",
    "jq",
    "hcl",
    "lua",
    "ruby",
    "php",
    "toml",
    "swift",
    "kotlin",
    "scala",
    "powershell",
    "elixir",
    "sql",
    "starlark",
    "objective-c",
    "xml",
    "vue",
    "dockerfile",
    "nix",
    "markdown",
    "nginx",
    "ini",
    "ssh-config",
    "diff",
    "awk",
    "jinja2",
    "caddy",
];

/// Registry that holds all of the supported languages.
pub struct LanguageRegistry {
    /// List of languages we support mapped from their display name. They are hold in Arc so they could be shared
    /// between different editors.
    languages: Mutex<HashMap<String, Arc<Language>>>,
}

impl LanguageRegistry {
    fn new() -> Self {
        Self {
            languages: Mutex::new(HashMap::new()),
        }
    }

    pub fn language_by_name(&self, name: &str) -> Option<Arc<Language>> {
        if !SUPPORTED_LANGUAGES.contains(&name) {
            return None;
        }

        let mut languages = self.languages.lock().expect("Mutex should not be poisoned");

        if let Some(lang) = languages.get(name) {
            return Some(lang.clone());
        }

        let language = Arc::new(load_language(name)?);
        languages.insert(name.to_string(), language.clone());
        Some(language)
    }
}

/// Find the corresponding language entry by a standardized filename.
pub fn language_by_filename(path: &StandardizedPath) -> Option<Arc<Language>> {
    language_by_filename_parts(path.file_name(), path.extension())
}

/// Find the corresponding language entry by a local filesystem filename.
pub fn language_by_local_filename(path: &Path) -> Option<Arc<Language>> {
    language_by_filename_parts(
        path.file_name().and_then(|file_name| file_name.to_str()),
        path.extension().and_then(|extension| extension.to_str()),
    )
}

/// Normalizes common language-name aliases to their canonical internal names.
/// For example, "go" -> "golang", "bash" -> "shell", "md" -> "markdown".
fn normalize_language_name(name: &str) -> &str {
    match name {
        "go" => "golang",
        "bash" | "sh" | "zsh" => "shell",
        "js" => "javascript",
        "ts" => "typescript",
        "py" => "python",
        "rb" => "ruby",
        "rs" => "rust",
        "cs" | "c#" => "csharp",
        "c++" => "cpp",
        "objc" | "objective_c" => "objective-c",
        "terraform" | "tf" => "hcl",
        "kt" => "kotlin",
        "docker" | "containerfile" => "dockerfile",
        "md" => "markdown",
        "j2" | "jinja" => "jinja2",
        "patch" => "diff",
        "ssh_config" | "sshconfig" => "ssh-config",
        "caddyfile" => "caddy",
        other => other,
    }
}

pub fn language_by_name(name: &str) -> Option<Arc<Language>> {
    let normalized = normalize_language_name(name);
    LANGUAGE_REGISTRY.language_by_name(normalized)
}

fn language_by_filename_parts(
    filename: Option<&str>,
    extension: Option<&str>,
) -> Option<Arc<Language>> {
    language_name_by_filename_parts(filename, extension).and_then(language_by_name)
}

/// The language for a file name and extension, when either is specific enough on its own.
fn language_name_by_filename_parts(
    filename: Option<&str>,
    extension: Option<&str>,
) -> Option<&'static str> {
    // First check for specific filenames that don't use extensions.
    if let Some(filename) = filename {
        match filename {
            // Bash config files
            ".bashrc" | ".bash_profile" | ".bash_aliases" | ".bash_logout" | ".profile" => {
                return Some("shell");
            }
            // ZSH config files
            ".zshrc" | ".zsh_profile" | ".zprofile" => {
                return Some("shell");
            }
            // Bazel build files
            "BUILD" | "WORKSPACE" => {
                return Some("starlark");
            }
            // Dockerfiles
            "Dockerfile" | "Containerfile" | "dockerfile" | "containerfile" => {
                return Some("dockerfile");
            }
            "nginx.conf" => return Some("nginx"),
            "sshd_config" | "ssh_config" => return Some("ssh-config"),
            "Caddyfile" => return Some("caddy"),
            _ => {
                // Also match Dockerfile variants like Dockerfile.dev, Dockerfile.prod
                if filename.starts_with("Dockerfile.") || filename.starts_with("Containerfile.") {
                    return Some("dockerfile");
                }
            }
        }
    }

    let extension = extension?;
    let name = match extension {
        "rs" => "rust",
        "go" => "golang",
        "yml" | "yaml" => "yaml",
        "py" | "py3" | "pyw" | "pyi" => "python",
        "js" | "cjs" | "mjs" => "javascript",
        "jsx" => "jsx",
        "tsx" => "tsx",
        "ts" | "cts" | "mts" => "typescript",
        "java" | "groovy" | "gvy" | "gy" | "gsh" => "java",
        "cpp" | "cxx" | "cc" | "h" | "hh" | "hpp" | "hxx" | "H" | "h++" => "cpp",
        "sh" | "zsh" | "bash" | "command" => "shell",
        "cs" => "csharp",
        "html" | "htm" => "html",
        "css" => "css",
        "c" => "c",
        "json" => "json",
        "jq" => "jq",
        "tf" | "hcl" | "tfvars" => "hcl",
        "lua" => "lua",
        "nix" => "nix",
        "rb" => "ruby",
        "php" | "phtml" => "php",
        "toml" => "toml",
        "swift" => "swift",
        "kt" | "kts" => "kotlin",
        "scala" | "sbt" | "sc" => "scala",
        "ps1" | "pwsh" => "powershell",
        "ex" | "exs" => "elixir",
        "sql" => "sql",
        "bzl" | "bazel" => "starlark",
        "m" | "mm" => "objective-c",
        "xml" => "xml",
        "vue" => "vue",
        "dockerfile" => "dockerfile",
        "md" | "markdown" => "markdown",
        // `.conf` is left out on purpose: nginx, Apache, HAProxy and many others share it.
        "ini" | "cnf" => "ini",
        // systemd units
        "service" | "timer" | "socket" | "mount" | "automount" | "target" | "path" | "slice"
        | "network" | "netdev" | "link" => "ini",
        "j2" | "jinja" | "jinja2" => "jinja2",
        "awk" => "awk",
        "diff" | "patch" => "diff",
        _ => return None,
    };
    Some(name)
}

/// Finds the language of a file from its path, which may be a path on another machine, and, for
/// a file that neither its name nor its folder identifies, from its first line (a `#!` line).
pub fn detect_language(path: &str, first_line: Option<&str>) -> Option<Arc<Language>> {
    detect_language_name(path, first_line).and_then(language_by_name)
}

/// The internal name of the language [`detect_language`] finds.
pub fn detect_language_name(path: &str, first_line: Option<&str>) -> Option<&'static str> {
    let path = path.replace('\\', "/");
    let filename = path.rsplit('/').next().filter(|name| !name.is_empty());
    let extension = filename.and_then(|name| {
        let (stem, extension) = name.rsplit_once('.')?;
        (!stem.is_empty()).then_some(extension)
    });
    language_name_by_filename_parts(filename, extension)
        .or_else(|| language_name_by_folder(&path, extension))
        .or_else(|| first_line.and_then(language_name_by_shebang))
}

/// Files that only their folder identifies: `.conf` files, and files without an extension, of
/// programs that keep their configuration in a folder of their own.
fn language_name_by_folder(path: &str, extension: Option<&str>) -> Option<&'static str> {
    let in_folder = |folder: &str| path.contains(folder);
    match extension {
        Some("conf") if in_folder("/nginx/") => Some("nginx"),
        Some("conf") if in_folder("/sshd_config.d/") || in_folder("/ssh_config.d/") => {
            Some("ssh-config")
        }
        Some("conf") if in_folder("/systemd/") => Some("ini"),
        None if in_folder("/nginx/sites-available/")
            || in_folder("/nginx/sites-enabled/")
            || in_folder("/nginx/conf.d/") =>
        {
            Some("nginx")
        }
        _ => None,
    }
}

/// The language of a script from its `#!` line, e.g. `#!/bin/bash` or `#!/usr/bin/env python3`.
fn language_name_by_shebang(first_line: &str) -> Option<&'static str> {
    let command = first_line.strip_prefix("#!")?;
    let mut words = command.split_whitespace();
    let mut program = words.next()?.rsplit('/').next()?;
    if program == "env" {
        program = words.find(|word| !word.starts_with('-'))?;
    }
    let program = program.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
    match program {
        "sh" | "bash" | "zsh" | "dash" | "ksh" | "ash" => Some("shell"),
        "python" => Some("python"),
        "ruby" => Some("ruby"),
        "node" | "nodejs" => Some("javascript"),
        "php" => Some("php"),
        "lua" => Some("lua"),
        "awk" | "gawk" | "mawk" | "nawk" => Some("awk"),
        _ => None,
    }
}

/// The internal name of a supported language, given that name or one of its aliases, e.g. `bash`.
pub fn supported_language_name(name: &str) -> Option<&'static str> {
    let normalized = normalize_language_name(name);
    SUPPORTED_LANGUAGES
        .iter()
        .copied()
        .find(|supported| *supported == normalized)
}

/// Every supported language as `(internal name, display name)`, sorted by display name. Only the
/// configuration files are read; no grammar is loaded.
pub fn supported_language_names() -> Vec<(&'static str, String)> {
    let mut names: Vec<(&'static str, String)> = SUPPORTED_LANGUAGES
        .iter()
        .map(|name| {
            let config = load_yaml(&[name, "config.yaml"].join("\\"));
            (*name, config.display_name)
        })
        .collect();
    names.sort_by_key(|(_, display_name)| display_name.to_lowercase());
    names
}

/// Captures the language-specific parser grammar and queries for syntax features like highlighting and
/// bracket pairing. In the future, this will also be the entry point for LSP.
pub struct Language {
    /// Tree-sitter parser grammar.
    pub grammar: ParserGrammar,
    /// Query for syntax highlighting.
    pub highlight_query: Query,
    /// Query for auto indent.
    pub indents_query: Option<Query>,
    /// Unit for each indent action.
    pub indent_unit: IndentUnit,
    /// Comment prefix.
    pub comment_prefix: Option<String>,
    /// Language-specific bracket pairs.
    pub bracket_pairs: Vec<(char, char)>,
    /// Query for parsing symbols.
    pub symbols_query: Option<Query>,
    /// Display name for the language.
    pub display_name: String,
}

impl Language {
    /// Returns the display name of the language.
    pub fn display_name(&self) -> &str {
        &self.display_name
    }
}

#[derive(Serialize, Deserialize, Debug)]
struct LanguageConfig {
    display_name: String,
    indent_unit: IndentUnit,
    comment_prefix: Option<String>,
    #[serde(default)]
    brackets: Vec<BracketPair>,
}

#[derive(Serialize, Deserialize, Debug)]
struct BracketPair {
    start: String,
    end: String,
}

/// Map our internal language name to the canonical arborium language name.
fn to_arborium_name(lang: &str) -> &str {
    match lang {
        "golang" => "go",
        "shell" => "bash",
        "csharp" => "c-sharp",
        "jsx" => "javascript",
        "objective-c" => "objc",
        "sql" => "sql",
        other => other,
    }
}

/// Get the bundled highlight query from arborium for a given language.
fn get_arborium_highlight_query(lang: &str) -> Option<&str> {
    match lang {
        "rust" => Some(arborium::lang_rust::HIGHLIGHTS_QUERY),
        "golang" => Some(arborium::lang_go::HIGHLIGHTS_QUERY),
        "yaml" => Some(arborium::lang_yaml::HIGHLIGHTS_QUERY),
        "python" => Some(arborium::lang_python::HIGHLIGHTS_QUERY),
        "javascript" => Some(arborium::lang_javascript::HIGHLIGHTS_QUERY),
        "jsx" => Some(arborium::lang_javascript::HIGHLIGHTS_QUERY),
        "typescript" => Some(&arborium::lang_typescript::HIGHLIGHTS_QUERY),
        "tsx" => Some(&arborium::lang_tsx::HIGHLIGHTS_QUERY),
        "java" => Some(arborium::lang_java::HIGHLIGHTS_QUERY),
        "cpp" => Some(&arborium::lang_cpp::HIGHLIGHTS_QUERY),
        "shell" => Some(arborium::lang_bash::HIGHLIGHTS_QUERY),
        "csharp" => Some(arborium::lang_c_sharp::HIGHLIGHTS_QUERY),
        "html" => Some(arborium::lang_html::HIGHLIGHTS_QUERY),
        "css" => Some(arborium::lang_css::HIGHLIGHTS_QUERY),
        "c" => Some(arborium::lang_c::HIGHLIGHTS_QUERY),
        "json" => Some(arborium::lang_json::HIGHLIGHTS_QUERY),
        "jq" => Some(arborium::lang_jq::HIGHLIGHTS_QUERY),
        "hcl" => Some(arborium::lang_hcl::HIGHLIGHTS_QUERY),
        "lua" => Some(arborium::lang_lua::HIGHLIGHTS_QUERY),
        "nix" => Some(arborium::lang_nix::HIGHLIGHTS_QUERY),
        "ruby" => Some(arborium::lang_ruby::HIGHLIGHTS_QUERY),
        "php" => Some(arborium::lang_php::HIGHLIGHTS_QUERY),
        "toml" => Some(arborium::lang_toml::HIGHLIGHTS_QUERY),
        "swift" => Some(arborium::lang_swift::HIGHLIGHTS_QUERY),
        "kotlin" => Some(arborium::lang_kotlin::HIGHLIGHTS_QUERY),
        "scala" => Some(arborium::lang_scala::HIGHLIGHTS_QUERY),
        "powershell" => Some(arborium::lang_powershell::HIGHLIGHTS_QUERY),
        "elixir" => Some(arborium::lang_elixir::HIGHLIGHTS_QUERY),
        "sql" => Some(arborium::lang_sql::HIGHLIGHTS_QUERY),
        "starlark" => Some(arborium::lang_starlark::HIGHLIGHTS_QUERY),
        "objective-c" => Some(&arborium::lang_objc::HIGHLIGHTS_QUERY),
        "xml" => Some(arborium::lang_xml::HIGHLIGHTS_QUERY),
        "vue" => Some(&arborium::lang_vue::HIGHLIGHTS_QUERY),
        "dockerfile" => Some(arborium::lang_dockerfile::HIGHLIGHTS_QUERY),
        "markdown" => Some(arborium::lang_markdown::HIGHLIGHTS_QUERY),
        "nginx" => Some(arborium::lang_nginx::HIGHLIGHTS_QUERY),
        "ini" => Some(arborium::lang_ini::HIGHLIGHTS_QUERY),
        "ssh-config" => Some(arborium::lang_ssh_config::HIGHLIGHTS_QUERY),
        "diff" => Some(arborium::lang_diff::HIGHLIGHTS_QUERY),
        "awk" => Some(arborium::lang_awk::HIGHLIGHTS_QUERY),
        "jinja2" => Some(arborium::lang_jinja2::HIGHLIGHTS_QUERY),
        "caddy" => Some(arborium::lang_caddy::HIGHLIGHTS_QUERY),
        _ => None,
    }
}

fn load_language(lang: &str) -> Option<Language> {
    let arborium_name = to_arborium_name(lang);
    let grammar = arborium::get_language(arborium_name)?;

    let config_path = [lang, "config.yaml"].join("\\");
    let config = load_yaml(&config_path);
    let indent_unit = config.indent_unit;
    let comment_prefix = config.comment_prefix;
    let bracket_pairs = config
        .brackets
        .into_iter()
        .filter_map(|bracket_pair| {
            let start = bracket_pair.start.chars().next()?;
            let end = bracket_pair.end.chars().next()?;
            Some((start, end))
        })
        .collect();

    let highlight_query_str = get_arborium_highlight_query(lang)?;
    let highlight_query = Query::new(&grammar, highlight_query_str)
        .expect("arborium highlight query should be valid");

    let indents_query_path = [lang, "indents.scm"].join("\\");
    let indents_query = load_query(&indents_query_path, &grammar);

    let symbols_query_path = [lang, "identifiers.scm"].join("\\");
    let symbols_query = load_query(&symbols_query_path, &grammar);

    Some(Language {
        highlight_query,
        indents_query,
        grammar,
        indent_unit,
        comment_prefix,
        bracket_pairs,
        symbols_query,
        display_name: config.display_name,
    })
}

fn load_yaml(path: &str) -> LanguageConfig {
    match <Grammars as RustEmbed>::get(path) {
        Some(file) => {
            let config: LanguageConfig =
                serde_yaml::from_slice(&file.data).expect("Unable to deserialize the YAML content");
            config
        }
        None => {
            panic!("Couldn't initiate yaml config from {path}");
        }
    }
}

fn load_query(path: &str, grammar: &ParserGrammar) -> Option<Query> {
    let file = <Grammars as RustEmbed>::get(path)?;
    let query_content = match file.data {
        Cow::Borrowed(inner) => Cow::Borrowed(std::str::from_utf8(inner).unwrap()),
        Cow::Owned(inner) => Cow::Owned(String::from_utf8(inner).unwrap()),
    };

    Some(
        Query::new(grammar, &query_content)
            .unwrap_or_else(|err| panic!("TSQuery creation should work from {path}: {err}")),
    )
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
