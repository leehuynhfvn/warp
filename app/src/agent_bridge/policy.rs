//! Agent-ops policy: decides whether an agent may run a write it asked for, independent of the
//! agent's own permission prompt. Pure evaluation lives here; `handlers::remote::authorize` is
//! the only caller.

use std::fs;
use std::io;
use std::path::Path;

use regex::Regex;
use serde::Deserialize;

use super::{APPROVAL_MAX_COMMAND_BYTES, APPROVAL_MAX_COMMAND_LINES, POLICY_FILE};

/// Characters that make an `allow` entry more than a single literal command, so it is rejected at
/// load time instead of being trusted to run unattended.
const DISALLOWED_ALLOW_CHARS: [char; 11] =
    [';', '|', '&', '$', '`', '<', '>', '(', ')', '\n', '\r'];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Mode {
    ReadOnly,
    Approve,
    Allowlist,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Rule {
    mode: Mode,
    allow: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HostRule {
    host_match: String,
    rule: Rule,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Policy {
    defaults: Rule,
    require_pairing: bool,
    hosts: Vec<HostRule>,
    /// Kept alongside its source text so a Deny reason can quote the pattern that matched.
    deny_patterns: Vec<(String, RegexEq)>,
    deny_paths: Vec<String>,
}

/// Wraps `Regex` so `Policy` can derive `PartialEq` for tests; regexes compare by source pattern.
#[derive(Debug, Clone)]
struct RegexEq(Regex);

impl PartialEq for RegexEq {
    fn eq(&self, other: &Self) -> bool {
        self.0.as_str() == other.0.as_str()
    }
}

impl Eq for RegexEq {}

#[derive(Debug, Clone, Copy)]
pub(crate) enum PolicyRequest<'a> {
    Exec(&'a str),
    ExecVisible(&'a str),
    Write { path: &'a str },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Decision {
    Allow,
    Ask,
    /// A reason suitable for the calling agent and the audit log.
    Deny(String),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum PolicyError {
    #[error("could not read the file: {0}")]
    Read(String),
    #[error("could not parse the file: {0}")]
    Parse(String),
    #[error("{0}")]
    Invalid(String),
    #[error(
        "its permissions ({mode:03o}) let the group or other users read or write it; run \
         'chmod 600' on it"
    )]
    Permissions { mode: u32 },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPolicy {
    defaults: RawRule,
    #[serde(default)]
    hosts: Vec<RawHostRule>,
    #[serde(default)]
    deny: RawDeny,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRule {
    mode: Mode,
    #[serde(default)]
    allow: Vec<String>,
    #[serde(default)]
    require_pairing: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawHostRule {
    #[serde(rename = "match")]
    host_match: String,
    mode: Mode,
    #[serde(default)]
    allow: Vec<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawDeny {
    #[serde(default)]
    patterns: Vec<String>,
    #[serde(default)]
    paths: Vec<String>,
}

impl Policy {
    /// Parses and validates policy source text (mục 2.3 of the plan). Takes `&str`, not a path,
    /// so tests can exercise it without touching disk.
    pub(crate) fn parse(text: &str) -> Result<Self, PolicyError> {
        let raw: RawPolicy =
            toml::from_str(text).map_err(|err| PolicyError::Parse(err.to_string()))?;

        let require_pairing = raw.defaults.require_pairing;
        let defaults = build_rule(raw.defaults.mode, raw.defaults.allow)?;

        let hosts = raw
            .hosts
            .into_iter()
            .map(|host| {
                if host.host_match.trim().is_empty() {
                    return Err(PolicyError::Invalid(
                        "a [[hosts]] entry has an empty 'match'".to_owned(),
                    ));
                }
                let rule = build_rule(host.mode, host.allow)?;
                Ok(HostRule { host_match: host.host_match, rule })
            })
            .collect::<Result<Vec<_>, _>>()?;

        let deny_patterns = raw
            .deny
            .patterns
            .into_iter()
            .map(|pattern| {
                Regex::new(&pattern)
                    .map(|regex| (pattern.clone(), RegexEq(regex)))
                    .map_err(|err| {
                        PolicyError::Invalid(format!(
                            "the deny pattern '{pattern}' does not compile: {err}"
                        ))
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self {
            defaults,
            require_pairing,
            hosts,
            deny_patterns,
            deny_paths: raw.deny.paths,
        })
    }

    /// Decides what to do with `request` on `hostname`, per the fixed rule order in mục 2.2 of
    /// the plan: pairing, then host selection, then `[deny]` (always wins), then the rule's mode.
    pub(crate) fn evaluate(
        &self,
        hostname: &str,
        request: PolicyRequest<'_>,
        paired: bool,
    ) -> Decision {
        if self.require_pairing && !paired {
            return Decision::Deny(
                "this agent is not paired with Warp. Ask the user to approve pairing, or set \
                 require_pairing = false in ~/.warp/agent-ops/policy.toml."
                    .to_owned(),
            );
        }

        if let Some(reason) = self.deny_reason(request) {
            return Decision::Deny(reason);
        }

        let rule = self.rule_for_host(hostname);
        match rule.mode {
            Mode::ReadOnly => Decision::Deny(format!("{hostname} is read-only for agents.")),
            Mode::Approve => ask_or_deny_for_length(request),
            Mode::Allowlist => match request {
                PolicyRequest::Exec(command) | PolicyRequest::ExecVisible(command)
                    if rule.allow.iter().any(|entry| entry == command.trim()) =>
                {
                    Decision::Allow
                }
                _ => ask_or_deny_for_length(request),
            },
        }
    }

    fn rule_for_host(&self, hostname: &str) -> &Rule {
        self.hosts
            .iter()
            .find(|host| glob_matches(&host.host_match, hostname, true))
            .map_or(&self.defaults, |host| &host.rule)
    }

    fn deny_reason(&self, request: PolicyRequest<'_>) -> Option<String> {
        match request {
            PolicyRequest::Exec(command) | PolicyRequest::ExecVisible(command) => self
                .deny_patterns
                .iter()
                .find(|(_, regex)| regex.0.is_match(command))
                .map(|(pattern, _)| deny_reason_text(pattern)),
            PolicyRequest::Write { path } => self
                .deny_paths
                .iter()
                .find(|glob| glob_matches(glob, path, false))
                .map(|pattern| deny_reason_text(pattern)),
        }
    }
}

impl Default for Policy {
    /// A missing policy file behaves like `[defaults] mode = "approve"` with no deny rules and no
    /// hosts: safe by default, but usable immediately.
    fn default() -> Self {
        Self {
            defaults: Rule { mode: Mode::Approve, allow: Vec::new() },
            require_pairing: false,
            hosts: Vec::new(),
            deny_patterns: Vec::new(),
            deny_paths: Vec::new(),
        }
    }
}

fn build_rule(mode: Mode, allow: Vec<String>) -> Result<Rule, PolicyError> {
    let allow = allow
        .into_iter()
        .map(|entry| {
            let trimmed = entry.trim();
            if trimmed.is_empty() {
                return Err(PolicyError::Invalid("an 'allow' entry is empty".to_owned()));
            }
            if let Some(bad) = trimmed.chars().find(|c| DISALLOWED_ALLOW_CHARS.contains(c)) {
                return Err(PolicyError::Invalid(format!(
                    "the 'allow' entry '{trimmed}' contains '{bad}', which allowlist entries \
                     cannot contain; write a script instead and allow running it"
                )));
            }
            Ok(trimmed.to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Rule { mode, allow })
}

fn deny_reason_text(pattern: &str) -> String {
    format!("it matches the deny rule '{pattern}'. This command is never allowed.")
}

/// `Ask`, unless `request` carries a command too long for a person to review (mục 2.2 step 5);
/// the length limit does not apply to `Write`, whose preview is truncated separately in the UI.
fn ask_or_deny_for_length(request: PolicyRequest<'_>) -> Decision {
    let command = match request {
        PolicyRequest::Exec(command) | PolicyRequest::ExecVisible(command) => Some(command),
        PolicyRequest::Write { .. } => None,
    };
    let Some(command) = command else {
        return Decision::Ask;
    };
    if command.len() > APPROVAL_MAX_COMMAND_BYTES || command.lines().count() > APPROVAL_MAX_COMMAND_LINES
    {
        return Decision::Deny(
            "it is too long for a person to review; write a script with write_file, then run it."
                .to_owned(),
        );
    }
    Decision::Ask
}

/// Minimal glob: `*` matches any run of characters (including none, including `/`), `?` matches
/// exactly one character. No other syntax is special.
fn glob_matches(pattern: &str, text: &str, case_insensitive: bool) -> bool {
    let normalize = |value: &str| if case_insensitive { value.to_lowercase() } else { value.to_owned() };
    let pattern: Vec<char> = normalize(pattern).chars().collect();
    let text: Vec<char> = normalize(text).chars().collect();

    let mut p = 0;
    let mut t = 0;
    let mut star_p = None;
    let mut star_t = 0;
    while t < text.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star_p = Some(p);
            star_t = t;
            p += 1;
        } else if let Some(sp) = star_p {
            p = sp + 1;
            star_t += 1;
            t = star_t;
        } else {
            return false;
        }
    }
    while pattern.get(p) == Some(&'*') {
        p += 1;
    }
    p == pattern.len()
}

/// Loads the policy from `home`'s `~/.warp/agent-ops/policy.toml`. A missing file is not an
/// error: it behaves like the safe default. Any other read/parse/permission failure is, so the
/// caller can deny every write until the user fixes it (fail-closed, mục 2.3 of the plan).
pub(crate) fn load(home: &Path) -> Result<Policy, PolicyError> {
    let path = home.join(POLICY_FILE);
    let metadata = match fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Policy::default()),
        Err(err) => return Err(PolicyError::Read(format!("{}: {err}", path.display()))),
    };
    check_permissions(&metadata)?;

    let text = fs::read_to_string(&path)
        .map_err(|err| PolicyError::Read(format!("{}: {err}", path.display())))?;
    Policy::parse(&text)
}

#[cfg(unix)]
fn check_permissions(metadata: &fs::Metadata) -> Result<(), PolicyError> {
    use std::os::unix::fs::PermissionsExt as _;

    let mode = metadata.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        return Err(PolicyError::Permissions { mode });
    }
    Ok(())
}

#[cfg(not(unix))]
fn check_permissions(_metadata: &fs::Metadata) -> Result<(), PolicyError> {
    Ok(())
}

#[cfg(test)]
#[path = "policy_tests.rs"]
mod tests;
