//! Which language a file in Warp's editor is highlighted with: the one the user chose for its path,
//! otherwise the one recognized from its name, folder or `#!` line.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock, PoisonError};

use crate::agent_bridge::policy::glob_matches;
use crate::settings::LanguageOverride;

/// The language name for `path`: the first override whose glob matches it and that names a
/// supported language, otherwise what `languages` recognizes. `first_line` lets a script without a
/// known name be recognized by its `#!` line.
pub fn language_for_path(
    overrides: &[LanguageOverride],
    path: &str,
    first_line: Option<&str>,
) -> Option<&'static str> {
    overrides
        .iter()
        .filter(|entry| glob_matches(&entry.glob, path, false))
        .find_map(|entry| supported_name(&entry.language))
        .or_else(|| languages::detect_language_name(path, first_line))
}

/// `overrides` with the choice for exactly `path` replaced: `Some` puts the language first, so
/// that it wins over broader globs; `None` removes the choice.
pub fn with_choice(
    overrides: &[LanguageOverride],
    path: &str,
    language: Option<&str>,
) -> Vec<LanguageOverride> {
    let chosen = language.map(|language| LanguageOverride {
        glob: path.to_owned(),
        language: language.to_owned(),
    });
    chosen
        .into_iter()
        .chain(overrides.iter().filter(|entry| entry.glob != path).cloned())
        .collect()
}

/// The internal name of a supported language, or `None` with a warning, once per name, for one
/// that is not supported.
fn supported_name(language: &str) -> Option<&'static str> {
    let name = languages::supported_language_name(language);
    if name.is_none() {
        static WARNED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
        let first_time = WARNED
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(language.to_owned());
        if first_time {
            log::warn!(
                "code.editor.language_overrides names an unsupported language: {language:?}"
            );
        }
    }
    name
}

#[cfg(test)]
#[path = "language_overrides_tests.rs"]
mod tests;
