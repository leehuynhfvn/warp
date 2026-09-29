//! The file Warp owns inside the SSH configuration, `~/.ssh/config.d/warp.conf`, and the one line
//! that makes OpenSSH read it. Every function here is pure: it takes the text of a file and gives
//! the new text, so that nothing but a checked, complete file is ever written.
//!
//! A host is a block of an optional `# warp:tags=a,b` comment and a `Host <alias>` line with its
//! indented options. Tags live in that comment so that `grep` finds them and OpenSSH ignores them.

use std::path::{Path, PathBuf};

use super::model::{parse_tags, validate_alias, validate_tag};
use super::ssh_config::tokenize;

const TAGS_COMMENT: &str = "# warp:tags=";
const HEADER: &str = "\
# Managed by Warp: hosts here can be edited in Settings > Servers.
# Anything you change by hand in a host block is kept.
";

/// The line that makes OpenSSH read `warp.conf`, relative to `~/.ssh` like every other `Include`.
pub(crate) const INCLUDE_LINE: &str = "Include config.d/warp.conf";

const MAX_HOSTNAME_LEN: usize = 255;
const MAX_USER_LEN: usize = 64;
const MAX_VALUE_LEN: usize = 1024;

/// What a person fills in to create a host. Only options that describe where to connect are
/// accepted: `ProxyCommand`, `LocalCommand` and `Match exec` would make the form a way to run
/// commands through the SSH configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct NewHost {
    pub(crate) alias: String,
    pub(crate) hostname: String,
    pub(crate) user: Option<String>,
    pub(crate) port: Option<u16>,
    pub(crate) identity_file: Option<String>,
    pub(crate) proxy_jump: Option<String>,
    pub(crate) tags: Vec<String>,
}

/// Refuses a host that could not be written as one plain block, saying which field is wrong.
pub(crate) fn validate(host: &NewHost) -> Result<(), String> {
    validate_alias(&host.alias).map_err(|reason| format!("The alias {reason}"))?;
    validate_hostname(&host.hostname)?;
    if let Some(user) = &host.user {
        validate_user(user)?;
    }
    if host.port == Some(0) {
        return Err("The port must be between 1 and 65535".to_owned());
    }
    if let Some(identity_file) = &host.identity_file {
        validate_word("The key file", identity_file, |c| {
            !c.is_whitespace() && !matches!(c, '"' | '\'' | '#')
        })?;
    }
    if let Some(proxy_jump) = &host.proxy_jump {
        validate_proxy_jump(proxy_jump)?;
    }
    for tag in &host.tags {
        validate_tag(tag).map_err(|reason| format!("The tag \"{tag}\" {reason}"))?;
    }
    parse_tags(&host.tags.join(",")).map(drop)
}

fn validate_hostname(hostname: &str) -> Result<(), String> {
    if hostname.len() > MAX_HOSTNAME_LEN {
        return Err("The host name is too long".to_owned());
    }
    validate_word("The host name", hostname, is_host_char)
}

fn validate_user(user: &str) -> Result<(), String> {
    if user.len() > MAX_USER_LEN {
        return Err("The user name is too long".to_owned());
    }
    validate_word("The user name", user, |c| {
        c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')
    })
}

/// `[user@]host[:port]` items, separated by commas.
fn validate_proxy_jump(value: &str) -> Result<(), String> {
    validate_word("The jump host", value, |c| {
        is_host_char(c) || matches!(c, '@' | ',')
    })
}

fn is_host_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':' | '%')
}

/// `value` must be non-empty, short, free of anything that is not a plain character, and must not
/// start with `-` (which `ssh` would take for an option).
fn validate_word(name: &str, value: &str, allowed: impl Fn(char) -> bool) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("{name} is empty"));
    }
    if value.len() > MAX_VALUE_LEN {
        return Err(format!("{name} is too long"));
    }
    if value.starts_with('-') {
        return Err(format!("{name} must not start with '-'"));
    }
    if value.chars().any(|c| c.is_control() || !allowed(c)) {
        return Err(format!("{name} contains a character that is not allowed"));
    }
    Ok(())
}

/// The block for `host`, ending in a newline.
pub(crate) fn render_block(host: &NewHost) -> String {
    let mut block = String::new();
    if let Some(comment) = tags_comment(&host.tags) {
        block.push_str(&comment);
        block.push('\n');
    }
    block.push_str(&format!("Host {}\n", host.alias));
    block.push_str(&format!("    HostName {}\n", host.hostname));
    if let Some(user) = &host.user {
        block.push_str(&format!("    User {user}\n"));
    }
    if let Some(port) = host.port {
        block.push_str(&format!("    Port {port}\n"));
    }
    if let Some(identity_file) = &host.identity_file {
        block.push_str(&format!("    IdentityFile {identity_file}\n"));
    }
    if let Some(proxy_jump) = &host.proxy_jump {
        block.push_str(&format!("    ProxyJump {proxy_jump}\n"));
    }
    block
}

fn tags_comment(tags: &[String]) -> Option<String> {
    (!tags.is_empty()).then(|| format!("{TAGS_COMMENT}{}", tags.join(",")))
}

/// A host block Warp can manage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WarpBlock {
    pub(crate) alias: String,
    pub(crate) tags: Vec<String>,
    /// The lines `[start, end)` of the file, from the tags comment (or the `Host` line) to the last
    /// non-blank line of the block.
    start: usize,
    end: usize,
    /// The line of the tags comment, if there is one.
    tags_line: Option<usize>,
}

/// The blocks Warp can manage in `text`: a `Host` line with exactly one valid alias. A block the
/// user wrote by hand with several aliases, and every `Match`, is left alone and not listed.
pub(crate) fn parse_blocks(text: &str) -> Vec<WarpBlock> {
    let lines: Vec<&str> = text.lines().collect();
    let mut blocks = Vec::new();
    let mut current: Option<WarpBlock> = None;

    for (index, raw) in lines.iter().enumerate() {
        let Some(line) = tokenize(raw) else {
            continue;
        };
        if line.keyword != "host" && line.keyword != "match" {
            continue;
        }
        if let Some(block) = current.take() {
            blocks.push(close_block(block, index, &lines));
        }
        let [alias] = line.args.as_slice() else {
            continue;
        };
        if line.keyword != "host" || validate_alias(alias).is_err() {
            continue;
        }
        let tags_line = index
            .checked_sub(1)
            .filter(|previous| lines[*previous].trim().starts_with(TAGS_COMMENT));
        let tags = tags_line
            .and_then(|previous| {
                let list = lines[previous].trim().strip_prefix(TAGS_COMMENT)?;
                parse_tags(list).ok()
            })
            .unwrap_or_default();
        current = Some(WarpBlock {
            alias: alias.clone(),
            tags,
            start: tags_line.unwrap_or(index),
            end: index + 1,
            tags_line,
        });
    }
    if let Some(block) = current.take() {
        blocks.push(close_block(block, lines.len(), &lines));
    }
    blocks
}

/// Ends `block` before line `next`, leaving the blank lines that separate it from the next block
/// outside it. A tags comment that belongs to the next block is not part of this one.
fn close_block(mut block: WarpBlock, next: usize, lines: &[&str]) -> WarpBlock {
    let mut end = next;
    while end > block.start + 1 && lines[end - 1].trim().is_empty() {
        end -= 1;
    }
    if end > block.start + 1 && lines[end - 1].trim().starts_with(TAGS_COMMENT) && end == next {
        end -= 1;
    }
    block.end = end;
    block
}

pub(crate) fn find_block(text: &str, alias: &str) -> Option<WarpBlock> {
    parse_blocks(text)
        .into_iter()
        .find(|block| block.alias == alias)
}

/// `text` with `block` added at the end, after a blank line. A new file starts with a header.
pub(crate) fn append_block(text: &str, block: &str) -> String {
    let mut result = String::new();
    if text.trim().is_empty() {
        result.push_str(HEADER);
    } else {
        result.push_str(text);
        if !text.ends_with('\n') {
            result.push('\n');
        }
    }
    result.push('\n');
    result.push_str(block);
    result
}

/// `text` without the block of `alias`, or `None` if there is none.
pub(crate) fn remove_block(text: &str, alias: &str) -> Option<String> {
    let block = find_block(text, alias)?;
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut kept: Vec<&str> = lines[..block.start].to_vec();
    let mut after = &lines[block.end..];
    // The blank line that separated the block from what preceded it would now sit next to the
    // one that separated it from what follows.
    let previous_is_blank = kept.last().is_none_or(|line| line.trim().is_empty());
    if previous_is_blank && after.first().is_some_and(|line| line.trim().is_empty()) {
        after = &after[1..];
    }
    if after.is_empty() {
        while kept.last().is_some_and(|line| line.trim().is_empty()) {
            kept.pop();
        }
    }
    kept.extend_from_slice(after);
    Some(kept.concat())
}

/// `text` with the tags of `alias` replaced, or `None` if there is no such block.
pub(crate) fn set_tags(text: &str, alias: &str, tags: &[String]) -> Option<String> {
    let block = find_block(text, alias)?;
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let host_line = block.tags_line.map_or(block.start, |line| line + 1);
    let mut result: Vec<String> = lines[..block.start]
        .iter()
        .map(|l| (*l).to_owned())
        .collect();
    if let Some(comment) = tags_comment(tags) {
        result.push(format!("{comment}\n"));
    }
    result.extend(lines[host_line..].iter().map(|l| (*l).to_owned()));
    Some(result.concat())
}

/// Whether the SSH configuration `text` reads `warp_conf` before its first `Host` or `Match`. An
/// `Include` after one of those would only apply to that block.
pub(crate) fn has_warp_include(text: &str, warp_conf: &Path, home: &Path) -> bool {
    for raw in text.lines() {
        let Some(line) = tokenize(raw) else {
            continue;
        };
        match line.keyword.as_str() {
            "host" | "match" => return false,
            "include" => {
                if line
                    .args
                    .iter()
                    .any(|arg| include_target(arg, home).as_deref() == Some(warp_conf))
                {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

fn include_target(arg: &str, home: &Path) -> Option<PathBuf> {
    if arg.contains('*') || arg.contains('?') || arg.contains('%') {
        return None;
    }
    let path = if let Some(rest) = arg.strip_prefix("~/") {
        home.join(rest)
    } else if Path::new(arg).is_absolute() {
        PathBuf::from(arg)
    } else {
        home.join(".ssh").join(arg)
    };
    Some(path.components().collect())
}

/// `text` with the `Include` line for `warp.conf` at the top, the only place where it applies to
/// every host. Nothing else in `text` changes.
pub(crate) fn with_warp_include(text: &str) -> String {
    if text.is_empty() {
        return format!("{INCLUDE_LINE}\n");
    }
    format!("{INCLUDE_LINE}\n\n{text}")
}

#[cfg(test)]
#[path = "warp_conf_tests.rs"]
mod tests;
