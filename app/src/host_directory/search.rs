//! Searching the server list from the command palette: words match aliases and tags loosely,
//! `tag:prod` keeps only hosts with that tag.

use fuzzy_match::{FuzzyMatchResult, match_indices_case_insensitive};

use super::model::{Host, validate_alias};

const TAG_PREFIX: &str = "tag:";

/// A tag match is a weaker signal than an alias match.
const TAG_SCORE_DIVISOR: i64 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ServerMatch<'a> {
    pub(crate) host: &'a Host,
    /// The indices are the characters of the alias that matched.
    pub(crate) fuzzy: FuzzyMatchResult,
}

/// Whether Warp can open a session to `host`: its alias is still in the SSH configuration and is
/// safe to type into a shell.
pub(crate) fn is_connectable(host: &Host) -> bool {
    !host.missing && validate_alias(&host.alias).is_ok()
}

/// The connectable hosts that match `query`, best first; every connectable host, by alias, when
/// the query is empty.
pub(crate) fn search<'a>(hosts: &'a [Host], query: &str) -> Vec<ServerMatch<'a>> {
    let mut required_tags = Vec::new();
    let mut words = Vec::new();
    for token in query.split_whitespace() {
        match strip_prefix_ignore_case(token, TAG_PREFIX) {
            Some("") => {}
            Some(tag) => required_tags.push(tag),
            None => words.push(token),
        }
    }

    let mut matches: Vec<ServerMatch<'a>> = hosts
        .iter()
        .filter(|host| is_connectable(host))
        .filter(|host| {
            required_tags
                .iter()
                .all(|tag| host.tags.iter().any(|own| own.eq_ignore_ascii_case(tag)))
        })
        .filter_map(|host| match_words(host, &words).map(|fuzzy| ServerMatch { host, fuzzy }))
        .collect();

    matches.sort_by(|a, b| {
        b.fuzzy
            .score
            .cmp(&a.fuzzy.score)
            .then_with(|| a.host.alias.cmp(&b.host.alias))
    });
    matches
}

/// What the server list in Settings shows: the hosts [`search`] finds, best first, then the hosts
/// that cannot be connected to (gone from the SSH configuration) whose alias contains `query`, so
/// that they can still be forgotten.
pub(crate) fn search_all<'a>(hosts: &'a [Host], query: &str) -> Vec<&'a Host> {
    let needle = query.trim().to_ascii_lowercase();
    let mut found: Vec<&Host> = search(hosts, query).into_iter().map(|m| m.host).collect();
    found.extend(
        hosts.iter().filter(|host| {
            !is_connectable(host) && host.alias.to_ascii_lowercase().contains(&needle)
        }),
    );
    found
}

/// Every word has to match the alias or one of the tags.
fn match_words(host: &Host, words: &[&str]) -> Option<FuzzyMatchResult> {
    let mut total = FuzzyMatchResult::no_match();
    for word in words {
        let found = match_indices_case_insensitive(&host.alias, word).or_else(|| {
            host.tags
                .iter()
                .filter_map(|tag| match_indices_case_insensitive(tag, word))
                .map(|tag_match| FuzzyMatchResult {
                    score: tag_match.score / TAG_SCORE_DIVISOR,
                    matched_indices: Vec::new(),
                })
                .max_by_key(|tag_match| tag_match.score)
        })?;
        total.score += found.score;
        total.matched_indices.extend(found.matched_indices);
    }
    total.matched_indices.sort_unstable();
    total.matched_indices.dedup();
    Some(total)
}

fn strip_prefix_ignore_case<'a>(token: &'a str, prefix: &str) -> Option<&'a str> {
    let head = token.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &token[prefix.len()..])
}

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;
