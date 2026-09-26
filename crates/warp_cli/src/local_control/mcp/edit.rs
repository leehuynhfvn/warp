//! String replacement for `edit_file`, done on the raw file content so that the model never needs
//! to see it.

/// A replacement that was applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Edit {
    pub content: String,
    pub replacements: usize,
    /// Byte offset in `content` where the first replacement starts.
    pub first_change: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EditError {
    EmptyOldString,
    Unchanged,
    NotFound,
    /// `old` occurs `count` times and only one replacement was asked for.
    Ambiguous {
        count: usize,
    },
}

/// Replaces `old` with `new` in `content`: exactly one occurrence, or every one with
/// `replace_all`.
pub(super) fn apply_edit(
    content: &str,
    old: &str,
    new: &str,
    replace_all: bool,
) -> Result<Edit, EditError> {
    if old.is_empty() {
        return Err(EditError::EmptyOldString);
    }
    if old == new {
        return Err(EditError::Unchanged);
    }
    let Some(first_change) = content.find(old) else {
        return Err(EditError::NotFound);
    };
    let count = content.matches(old).count();
    if count > 1 && !replace_all {
        return Err(EditError::Ambiguous { count });
    }
    Ok(Edit {
        content: content.replace(old, new),
        replacements: count,
        first_change,
    })
}

#[cfg(test)]
#[path = "edit_tests.rs"]
mod tests;
