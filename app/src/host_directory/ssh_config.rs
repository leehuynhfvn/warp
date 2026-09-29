//! Finds the concrete host aliases in the user's SSH configuration. Only the names are read:
//! what an alias connects to is left to `ssh -G`, which resolves `Include`, `Match` and wildcards
//! the way OpenSSH itself does.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::{fs, io};

use super::model::{DiscoveredHost, validate_alias};
use crate::agent_bridge::policy::glob_matches;

/// The depth at which OpenSSH itself stops following `Include`.
const MAX_INCLUDE_DEPTH: usize = 16;
const MAX_FILES: usize = 256;
const MAX_FILE_BYTES: u64 = 1024 * 1024;
const MAX_ALIASES: usize = 10_000;
const MAX_GLOB_MATCHES: usize = 1024;

/// What a scan of the SSH configuration found.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Discovery {
    pub(crate) aliases: Vec<DiscoveredHost>,
    /// Every file that was read, so that a later change to any of them can be noticed.
    pub(crate) files: Vec<PathBuf>,
    /// Things that were skipped, worth a line in the log.
    pub(crate) warnings: Vec<String>,
}

/// Scans `config` and the files it includes. A relative `Include` is resolved from `ssh_dir` and
/// `~` stands for `home`, as in OpenSSH. A missing `config` is an empty result.
pub(crate) fn discover_aliases(config: &Path, ssh_dir: &Path, home: &Path) -> Discovery {
    let mut scan = Scan {
        ssh_dir,
        home,
        found: Discovery::default(),
        seen_aliases: HashSet::new(),
        stack: Vec::new(),
    };
    scan.file(config, 0);
    scan.found
}

struct Scan<'a> {
    ssh_dir: &'a Path,
    home: &'a Path,
    found: Discovery,
    seen_aliases: HashSet<String>,
    /// The files being read right now, to stop an `Include` that leads back to itself.
    stack: Vec<PathBuf>,
}

impl Scan<'_> {
    fn file(&mut self, path: &Path, depth: usize) {
        if depth > MAX_INCLUDE_DEPTH {
            self.warn(format!("{}: Include is nested too deeply", path.display()));
            return;
        }
        if self.found.files.len() >= MAX_FILES {
            self.warn(format!("{}: too many files, skipped", path.display()));
            return;
        }
        let identity = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
        if self.stack.contains(&identity) {
            self.warn(format!("{}: Include leads back to itself", path.display()));
            return;
        }
        let Some(text) = self.read(path) else {
            return;
        };
        self.found.files.push(path.to_owned());
        self.stack.push(identity);
        for (index, raw) in text.lines().enumerate() {
            let Some(line) = tokenize(raw) else {
                continue;
            };
            match line.keyword.as_str() {
                "host" => self.hosts(&line.args, path, index + 1),
                "include" => self.includes(&line.args, path, depth),
                _ => {}
            }
        }
        self.stack.pop();
    }

    fn read(&mut self, path: &Path) -> Option<String> {
        match fs::metadata(path) {
            Ok(meta) if meta.len() > MAX_FILE_BYTES => {
                self.warn(format!("{}: larger than 1 MiB, skipped", path.display()));
                return None;
            }
            Ok(_) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => return None,
            Err(err) => {
                self.warn(format!("{}: {err}", path.display()));
                return None;
            }
        }
        match fs::read(path) {
            Ok(bytes) => Some(String::from_utf8_lossy(&bytes).into_owned()),
            Err(err) => {
                self.warn(format!("{}: {err}", path.display()));
                None
            }
        }
    }

    fn hosts(&mut self, patterns: &[String], file: &Path, line: usize) {
        for pattern in patterns {
            if pattern.contains(['*', '?', '!']) {
                continue;
            }
            if let Err(reason) = validate_alias(pattern) {
                self.warn(format!(
                    "{}:{line}: the host \"{pattern}\" {reason}, skipped",
                    file.display()
                ));
                continue;
            }
            if self.found.aliases.len() >= MAX_ALIASES {
                self.warn(format!(
                    "more than {MAX_ALIASES} hosts, the rest are skipped"
                ));
                return;
            }
            if self.seen_aliases.insert(pattern.clone()) {
                self.found.aliases.push(DiscoveredHost {
                    alias: pattern.clone(),
                    file: file.to_owned(),
                    line,
                });
            }
        }
    }

    fn includes(&mut self, patterns: &[String], from: &Path, depth: usize) {
        for pattern in patterns {
            let Some(pattern_path) = self.include_path(pattern) else {
                self.warn(format!(
                    "{}: cannot follow Include {pattern}, skipped",
                    from.display()
                ));
                continue;
            };
            for path in expand_glob(&pattern_path) {
                self.file(&path, depth + 1);
            }
        }
    }

    /// The absolute path pattern an `Include` argument stands for, or `None` for one that needs
    /// expansions (`%d`, `${VAR}`, `~user`) that are not supported.
    fn include_path(&self, pattern: &str) -> Option<PathBuf> {
        if pattern.contains('%') || pattern.contains("${") {
            return None;
        }
        if pattern == "~" {
            return Some(self.home.to_owned());
        }
        if let Some(rest) = pattern.strip_prefix("~/") {
            return Some(self.home.join(rest));
        }
        if pattern.starts_with('~') {
            return None;
        }
        Some(self.ssh_dir.join(pattern))
    }

    fn warn(&mut self, message: String) {
        self.found.warnings.push(message);
    }
}

/// One configuration line: its keyword, lowercased, and its arguments.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Line {
    pub(super) keyword: String,
    pub(super) args: Vec<String>,
}

/// Splits a line like OpenSSH does: the keyword and its arguments are separated by white space or
/// by a single `=`, an argument may be double-quoted, and a `#` at the start of a word begins a
/// comment. `None` for an empty or comment line.
pub(super) fn tokenize(raw: &str) -> Option<Line> {
    let mut rest = raw.trim_start();
    if rest.is_empty() || rest.starts_with('#') {
        return None;
    }
    let keyword_end = rest
        .find(|c: char| c.is_whitespace() || c == '=')
        .unwrap_or(rest.len());
    let keyword = rest[..keyword_end].to_ascii_lowercase();
    rest = rest[keyword_end..].trim_start();
    if let Some(after_equals) = rest.strip_prefix('=') {
        rest = after_equals;
    }

    let mut args = Vec::new();
    let mut chars = rest.chars().peekable();
    loop {
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        match chars.peek() {
            None | Some('#') => break,
            Some(_) => {}
        }
        let mut arg = String::new();
        let mut quoted = false;
        while let Some(&c) = chars.peek() {
            if !quoted && c.is_whitespace() {
                break;
            }
            chars.next();
            match c {
                '"' => quoted = !quoted,
                '\\' if quoted && chars.peek() == Some(&'"') => {
                    arg.push('"');
                    chars.next();
                }
                _ => arg.push(c),
            }
        }
        args.push(arg);
    }
    Some(Line { keyword, args })
}

/// The existing files that `pattern` names, in the sorted order `glob(3)` gives. Only `*` and `?`
/// are wildcards and only within one path component; like the shell, they do not match a name
/// that starts with a dot.
fn expand_glob(pattern: &Path) -> Vec<PathBuf> {
    let mut paths = vec![PathBuf::new()];
    for component in pattern.components() {
        let name = component.as_os_str().to_string_lossy();
        let is_wildcard = matches!(component, Component::Normal(_)) && name.contains(['*', '?']);
        if !is_wildcard {
            paths = paths
                .into_iter()
                .map(|path| path.join(component.as_os_str()))
                .collect();
            continue;
        }
        let mut matches = Vec::new();
        for dir in &paths {
            let Ok(entries) = fs::read_dir(dir) else {
                continue;
            };
            let mut names: Vec<_> = entries
                .filter_map(Result::ok)
                .map(|entry| entry.file_name())
                .collect();
            names.sort();
            matches.extend(
                names
                    .into_iter()
                    .filter(|entry| component_matches(&name, &entry.to_string_lossy()))
                    .map(|entry| dir.join(entry)),
            );
            matches.truncate(MAX_GLOB_MATCHES);
        }
        paths = matches;
    }
    paths.retain(|path| path.is_file());
    paths
}

fn component_matches(pattern: &str, name: &str) -> bool {
    if name.starts_with('.') && !pattern.starts_with('.') {
        return false;
    }
    glob_matches(pattern, name, false)
}

#[cfg(test)]
#[path = "ssh_config_tests.rs"]
mod tests;
