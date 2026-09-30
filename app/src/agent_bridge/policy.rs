//! Agent-ops policy: decides whether an agent may run a write it asked for, independent of the
//! agent's own permission prompt. Pure evaluation lives here; `handlers::remote::authorize` is
//! the only caller.

use std::path::Path;
use std::{fs, io};

use regex::Regex;
use serde::Deserialize;

use super::attachments::Access;
use super::{
    APPROVAL_MAX_COMMAND_BYTES, APPROVAL_MAX_COMMAND_LINES, OPEN_DEFAULT_MAX_PER_AGENT,
    OPEN_DEFAULT_MAX_PER_HOST, OPEN_MAX_PER_AGENT, OPEN_MAX_PER_HOST, POLICY_FILE,
};

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
    open_hosts: Vec<OpenHostRule>,
    open_limits: OpenLimits,
}

/// What an agent may do about opening sessions on a server the policy names (`[[open.hosts]]`).
#[derive(Debug, Clone, PartialEq, Eq)]
struct OpenHostRule {
    host_match: String,
    tag: Option<String>,
    mode: OpenMode,
    max_access: Access,
    allow_root: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum OpenMode {
    Allow,
    Ask,
    Deny,
}

/// How many sessions one agent may hold open, on one server and in all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OpenLimits {
    pub(crate) per_host: usize,
    pub(crate) per_agent: usize,
}

impl Default for OpenLimits {
    fn default() -> Self {
        Self {
            per_host: OPEN_DEFAULT_MAX_PER_HOST,
            per_agent: OPEN_DEFAULT_MAX_PER_AGENT,
        }
    }
}

/// An agent asking to open a session on its own.
#[derive(Debug, Clone, Copy)]
pub(crate) struct OpenRequest<'a> {
    pub(crate) alias: &'a str,
    pub(crate) tags: &'a [String],
    pub(crate) access: Access,
    /// The agent wants a root shell, not just the login user's.
    pub(crate) root: bool,
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
    #[serde(default)]
    open: RawOpen,
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
struct RawOpen {
    max_sessions_per_host: Option<usize>,
    max_sessions_per_agent: Option<usize>,
    #[serde(default)]
    hosts: Vec<RawOpenHost>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawOpenHost {
    #[serde(rename = "match")]
    host_match: String,
    #[serde(default)]
    tag: Option<String>,
    mode: OpenMode,
    #[serde(default)]
    max_access: RawAccess,
    #[serde(default)]
    allow_root: bool,
}

#[derive(Deserialize, Default, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum RawAccess {
    #[default]
    ReadOnly,
    Full,
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
                Ok(HostRule {
                    host_match: host.host_match,
                    rule,
                })
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

        let open_limits = OpenLimits {
            per_host: open_limit(
                "max_sessions_per_host",
                raw.open.max_sessions_per_host,
                OPEN_DEFAULT_MAX_PER_HOST,
                OPEN_MAX_PER_HOST,
            )?,
            per_agent: open_limit(
                "max_sessions_per_agent",
                raw.open.max_sessions_per_agent,
                OPEN_DEFAULT_MAX_PER_AGENT,
                OPEN_MAX_PER_AGENT,
            )?,
        };
        let open_hosts = raw
            .open
            .hosts
            .into_iter()
            .map(build_open_host)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self {
            defaults,
            require_pairing,
            hosts,
            deny_patterns,
            deny_paths: raw.deny.paths,
            open_hosts,
            open_limits,
        })
    }

    pub(crate) fn open_limits(&self) -> OpenLimits {
        self.open_limits
    }

    /// Decides whether an agent may open a session on `request.alias`, per the first
    /// `[[open.hosts]]` rule that names it. A server no rule names is asked about: nothing opens
    /// on its own unless the user wrote a rule saying it may. An `allow` rule does not cover more
    /// than its `max_access`, or a root shell it does not `allow_root` for: those are asked about
    /// too, never silently lowered.
    pub(crate) fn evaluate_open(&self, request: OpenRequest<'_>) -> Decision {
        let Some(rule) = self.open_hosts.iter().find(|rule| rule.covers(request)) else {
            return Decision::Ask;
        };
        match rule.mode {
            OpenMode::Deny => Decision::Deny(format!(
                "the policy does not let agents open sessions on {}.",
                request.alias
            )),
            OpenMode::Ask => Decision::Ask,
            OpenMode::Allow => {
                if request.access > rule.max_access || (request.root && !rule.allow_root) {
                    Decision::Ask
                } else {
                    Decision::Allow
                }
            }
        }
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
            defaults: Rule {
                mode: Mode::Approve,
                allow: Vec::new(),
            },
            require_pairing: false,
            hosts: Vec::new(),
            deny_patterns: Vec::new(),
            deny_paths: Vec::new(),
            open_hosts: Vec::new(),
            open_limits: OpenLimits::default(),
        }
    }
}

impl OpenHostRule {
    fn covers(&self, request: OpenRequest<'_>) -> bool {
        glob_matches(&self.host_match, request.alias, true)
            && self
                .tag
                .as_ref()
                .is_none_or(|tag| request.tags.iter().any(|candidate| candidate == tag))
    }
}

fn open_limit(
    key: &str,
    value: Option<usize>,
    default: usize,
    cap: usize,
) -> Result<usize, PolicyError> {
    match value {
        None => Ok(default),
        Some(value) if (1..=cap).contains(&value) => Ok(value),
        Some(_) => Err(PolicyError::Invalid(format!(
            "[open] {key} must be between 1 and {cap}"
        ))),
    }
}

fn build_open_host(raw: RawOpenHost) -> Result<OpenHostRule, PolicyError> {
    if raw.host_match.trim().is_empty() {
        return Err(PolicyError::Invalid(
            "an [[open.hosts]] entry has an empty 'match'".to_owned(),
        ));
    }
    let tag = match raw.tag {
        Some(tag) if tag.trim().is_empty() => {
            return Err(PolicyError::Invalid(
                "an [[open.hosts]] entry has an empty 'tag'".to_owned(),
            ));
        }
        tag => tag.map(|tag| tag.trim().to_owned()),
    };
    Ok(OpenHostRule {
        host_match: raw.host_match,
        tag,
        mode: raw.mode,
        max_access: match raw.max_access {
            RawAccess::ReadOnly => Access::ReadOnly,
            RawAccess::Full => Access::Full,
        },
        allow_root: raw.allow_root,
    })
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
    if command.len() > APPROVAL_MAX_COMMAND_BYTES
        || command.lines().count() > APPROVAL_MAX_COMMAND_LINES
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
pub(crate) fn glob_matches(pattern: &str, text: &str, case_insensitive: bool) -> bool {
    let normalize = |value: &str| {
        if case_insensitive {
            value.to_lowercase()
        } else {
            value.to_owned()
        }
    };
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

/// Why every write is refused while the policy file cannot be loaded, for the agent.
pub(crate) fn invalid_policy_reason(error: &PolicyError) -> String {
    format!(
        "the policy file ~/.warp/agent-ops/policy.toml is invalid ({error}). Ask the user to fix \
         it."
    )
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
