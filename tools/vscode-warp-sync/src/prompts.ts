// The text of the confirmation dialogs. Warp asks for a confirmation before anything is
// overwritten; these functions say what is about to happen.
import { UploadSummary } from "./protocol";

const MAX_LISTED_PATHS = 10;
const BYTES_PER_KIB = 1024;
const BYTES_PER_MIB = 1024 * 1024;

export interface Prompt {
  title: string;
  detail: string;
  confirmLabel: string;
  /** Whether going ahead would overwrite something the user has not seen. */
  hasWarnings: boolean;
}

export function count(amount: number, noun: string): string {
  return `${amount} ${noun}${amount === 1 ? "" : "s"}`;
}

export function formatSize(bytes: number): string {
  if (bytes >= BYTES_PER_MIB) {
    return `${(bytes / BYTES_PER_MIB).toFixed(1)} MiB`;
  }
  if (bytes >= BYTES_PER_KIB) {
    return `${(bytes / BYTES_PER_KIB).toFixed(1)} KiB`;
  }
  return `${bytes} B`;
}

function listPaths(paths: readonly string[]): string[] {
  const lines = paths.slice(0, MAX_LISTED_PATHS).map((path) => `    ${path}`);
  if (paths.length > MAX_LISTED_PATHS) {
    lines.push(`    … and ${paths.length - MAX_LISTED_PATHS} more`);
  }
  return lines;
}

function section(heading: string, paths: readonly string[]): string[] {
  return paths.length === 0 ? [] : ["", heading, ...listPaths(paths)];
}

/** The new entries, each followed by its mode where Warp reports one. */
function newEntries(summary: UploadSummary): string[] {
  return withModes(summary.new_files, summary.new_file_modes);
}

function withModes(paths: readonly string[], modes: Record<string, string> = {}): string[] {
  return paths.map((path) => {
    const mode = Object.hasOwn(modes, path) ? modes[path] : undefined;
    return mode === undefined ? path : `${path} (mode ${mode})`;
  });
}

export function uploadPrompt(summary: UploadSummary): Prompt {
  const conflicts = summary.remote_conflicts;
  const warnings: string[] = [];
  if (conflicts === undefined) {
    warnings.push(
      "",
      "WARNING: the server could not be checked for changes made since the last sync.",
    );
  } else {
    warnings.push(
      ...section(
        "WARNING: changed on the server since the last sync (the upload overwrites them):",
        conflicts.changed,
      ),
      ...section("WARNING: synced before but gone from the server:", conflicts.missing),
      ...section(
        "WARNING: new here but already on the server (the upload overwrites them):",
        conflicts.already_exist,
      ),
    );
  }
  warnings.push(
    ...section(
      "WARNING: anyone on the server could change these new entries (their mode allows write for others):",
      withModes(summary.world_writable ?? [], summary.new_file_modes),
    ),
    ...section(
      "WARNING: these new entries are where the server runs or trusts what it finds (cron, sudoers, shell startup files, ssh keys, services, program directories):",
      summary.runs_code ?? [],
    ),
  );
  const target = `${summary.remote_user}@${summary.hostname}`;
  const machine = summary.server_id_tail === undefined ? "" : ` (machine id ending ${summary.server_id_tail})`;
  const lines = [
    `${count(summary.files, "file")} and ${count(summary.dirs, "folder")} (${formatSize(summary.bytes)}) will be written on ${target}${machine}, as ${summary.remote_user}.`,
    summary.creates_under === undefined
      ? "A backup of what is replaced is kept on the server."
      : "Nothing on the server is replaced, so no backup is made.",
    ...section(
      summary.creates_under === undefined
        ? "New files:"
        : `Creates on the server, inside ${summary.creates_under}:`,
      newEntries(summary),
    ),
    ...section(
      "Missing from the mirror (they will not be deleted on the server):",
      summary.missing_locally,
    ),
    ...warnings,
  ];
  if (summary.ownership_may_be_incomplete) {
    lines.push("", "The server's tar is not GNU tar, so ownership may not be restored completely.");
  }
  const hasWarnings = warnings.length > 0;
  return {
    title: `Upload ${summary.remote_path} to ${target}?`,
    detail: lines.join("\n"),
    confirmLabel: hasWarnings ? "Upload Anyway" : "Upload",
    hasWarnings,
  };
}

export function overwritePrompt(files: readonly string[]): Prompt {
  return {
    title: "Overwrite local changes?",
    detail: [
      `Downloading replaces ${count(files.length, "file")} that you changed in the mirror and did not upload:`,
      ...listPaths(files),
    ].join("\n"),
    confirmLabel: "Overwrite",
    hasWarnings: true,
  };
}
