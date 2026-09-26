//! Hides secrets in everything the MCP server shows the model.
//!
//! Warp's default secret patterns also match IP and MAC addresses and phone numbers; those are
//! left out here because server administration is mostly about exactly such addresses, and a
//! config file whose addresses read `*****` cannot be edited. Patterns for the credentials that
//! server config files hold are added instead.
use std::borrow::Cow;

use regex::Regex;
use secret_redaction::regexes::{
    DEFAULT_REGEXES_WITH_NAMES, FIREBASE_AUTH_DOMAIN, IPV4_ADDRESS, IPV6_ADDRESS, MAC_ADDRESS,
    PHONE_NUMBER,
};

/// Default patterns that match addresses rather than credentials.
const NOT_SECRETS: &[&str] = &[
    IPV4_ADDRESS,
    IPV6_ADDRESS,
    PHONE_NUMBER,
    MAC_ADDRESS,
    FIREBASE_AUTH_DOMAIN,
];

/// Patterns for credentials in config files and environments. Only the `secret` group is hidden
/// so that the model still sees which setting holds a secret.
const CONFIG_SECRETS: &[&str] = &[
    // `DB_PASSWORD=…`, `"api_key": "…"`, `aws_secret_access_key = …`.
    r#"(?i)\b[\w.-]*(?:password|passwd|secret|token|api[_-]?key|access[_-]?key|private[_-]?key|credentials?)[\w.-]*["']?[ \t]*[:=][ \t]*["']?(?P<secret>[^\s"',;]+)"#,
    // The password in `scheme://user:password@host`.
    r"\b[a-zA-Z][a-zA-Z0-9+.-]*://[^\s:/@]+:(?P<secret>[^\s@/]+)@",
    // The body of a PEM private key.
    r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----(?P<secret>.+?)-----END [A-Z0-9 ]*PRIVATE KEY-----",
];

const SECRET_GROUP: &str = "secret";

const REPLACEMENT: char = '*';

/// The secret patterns used for text shown to the model.
pub(super) struct Redactor {
    patterns: Vec<Regex>,
}

impl Redactor {
    pub fn with_default_patterns() -> Self {
        let default_patterns = DEFAULT_REGEXES_WITH_NAMES
            .iter()
            .map(|default| default.pattern)
            .filter(|pattern| !NOT_SECRETS.contains(pattern));
        let patterns = default_patterns
            .chain(CONFIG_SECRETS.iter().copied())
            .filter_map(|pattern| match Regex::new(pattern) {
                Ok(regex) => Some(regex),
                Err(err) => {
                    eprintln!("warpctrl mcp: ignoring a secret pattern that does not compile: {err}");
                    None
                }
            })
            .collect();
        Self { patterns }
    }

    /// For `--no-redact`.
    pub fn disabled() -> Self {
        Self {
            patterns: Vec::new(),
        }
    }

    /// `text` with every secret replaced by `*`, one per byte. Byte offsets and line breaks stay
    /// where they were, so line numbers still match the file.
    pub fn to_model_text<'a>(&self, text: &'a str) -> Cow<'a, str> {
        let mut hidden = vec![false; text.len()];
        let mut found = false;
        for pattern in &self.patterns {
            for captures in pattern.captures_iter(text) {
                let Some(secret) = captures.name(SECRET_GROUP).or_else(|| captures.get(0)) else {
                    continue;
                };
                hidden[secret.range()].fill(true);
                found |= !secret.is_empty();
            }
        }
        if !found {
            return Cow::Borrowed(text);
        }
        let mut redacted = String::with_capacity(text.len());
        for (index, character) in text.char_indices() {
            if hidden[index] && character != '\n' {
                redacted.extend(std::iter::repeat_n(REPLACEMENT, character.len_utf8()));
            } else {
                redacted.push(character);
            }
        }
        Cow::Owned(redacted)
    }

    pub fn contains_secrets(&self, text: &str) -> bool {
        matches!(self.to_model_text(text), Cow::Owned(_))
    }
}

#[cfg(test)]
#[path = "redact_tests.rs"]
mod tests;
