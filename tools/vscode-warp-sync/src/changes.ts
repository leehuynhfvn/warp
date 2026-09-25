// Which files of the Git working tree are worth uploading.

// Status numbers of the built-in Git extension whose files no longer exist or are not tracked on
// purpose: INDEX_DELETED, DELETED, IGNORED, DELETED_BY_US, DELETED_BY_THEM, BOTH_DELETED.
const NOT_UPLOADABLE = new Set([2, 6, 8, 14, 15, 17]);

export interface GitChange {
  path: string;
  status: number;
}

/** The changed files that exist, without duplicates, in a stable order. */
export function uploadableChanges(changes: readonly GitChange[]): string[] {
  const paths = changes
    .filter((change) => !NOT_UPLOADABLE.has(change.status))
    .map((change) => change.path);
  return [...new Set(paths)].sort();
}
