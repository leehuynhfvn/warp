// The result shapes of `warpctrl sync ... --output-format json`
// (`SyncResult` in crates/local_control/src/protocol.rs). Strings that come from the server have
// already been made printable by Warp.

export interface SyncSession {
  session_id: string;
  window_id: string;
  tab_index: number;
  hostname: string;
  user: string;
  is_active: boolean;
}

export interface PathStatus {
  host_key: string;
  remote_path?: string;
  sessions: SyncSession[];
}

export interface RemoteConflicts {
  changed: string[];
  missing: string[];
  already_exist: string[];
}

export interface UploadSummary {
  remote_user: string;
  hostname: string;
  remote_path: string;
  files: number;
  dirs: number;
  bytes: number;
  new_files: string[];
  /** Octal permission bits, such as `0644`, that each of `new_files` gets on the server. */
  new_file_modes?: Record<string, string>;
  /** The synced directory the path is created in, when it is not on the server yet. */
  creates_under?: string;
  /** New entries that anyone on the server could change. */
  world_writable?: string[];
  /** New entries where the server runs or trusts what it finds (cron, sudoers, ssh keys, …). */
  runs_code?: string[];
  missing_locally: string[];
  remote_conflicts?: RemoteConflicts;
  ownership_may_be_incomplete: boolean;
  server_id_tail?: string;
}

export interface SkippedEntry {
  path: string;
  reason: string;
}

export interface Difference {
  remote_path: string;
  change: string;
  on_both_sides: boolean;
}

export interface StatusResult {
  status: "status";
  mirror_root: string;
  path?: PathStatus;
}

export interface DownloadedResult {
  status: "downloaded";
  local_path: string;
  files: number;
  dirs: number;
  bytes: number;
  remote_user: string;
  skipped: SkippedEntry[];
  baseline_warning?: string;
}

export interface OverwriteConfirmation {
  status: "needs_confirmation";
  pending_id: string;
  kind: "overwrite_local_changes";
  files: string[];
}

export interface UploadConfirmation {
  status: "needs_confirmation";
  pending_id: string;
  kind: "upload";
  summary: UploadSummary;
}

export type ConfirmationResult = OverwriteConfirmation | UploadConfirmation;

export interface UploadedResult {
  status: "uploaded";
  files: number;
  dirs: number;
  bytes: number;
  remote_user: string;
  backup_path?: string;
  baseline_warning?: string;
}

export interface ComparedResult {
  status: "compared";
  differences: Difference[];
  identical_files: number;
  diff_path: string;
  host_dir: string;
  server_copy_dir: string;
  remote_user: string;
}

export interface UnchangedResult {
  status: "unchanged";
  identical_files: number;
}

export interface CancelledResult {
  status: "cancelled";
}

export type SyncResult =
  | StatusResult
  | DownloadedResult
  | ConfirmationResult
  | UploadedResult
  | ComparedResult
  | UnchangedResult
  | CancelledResult;

type Json = Record<string, unknown>;

function isObject(value: unknown): value is Json {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isString(value: unknown): value is string {
  return typeof value === "string";
}

function isNumber(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value);
}

function isStringArray(value: unknown): value is string[] {
  return Array.isArray(value) && value.every(isString);
}

function isStringRecord(value: unknown): value is Record<string, string> {
  return isObject(value) && Object.values(value).every(isString);
}

// Warp leaves an absent field out; `null` is not something it sends.
function isOptional(value: unknown, check: (value: unknown) => boolean): boolean {
  return value === undefined || check(value);
}

function isSession(value: unknown): value is SyncSession {
  return (
    isObject(value) &&
    isString(value.session_id) &&
    isString(value.window_id) &&
    isNumber(value.tab_index) &&
    isString(value.hostname) &&
    isString(value.user) &&
    typeof value.is_active === "boolean"
  );
}

function isConflicts(value: unknown): value is RemoteConflicts {
  return (
    isObject(value) &&
    isStringArray(value.changed) &&
    isStringArray(value.missing) &&
    isStringArray(value.already_exist)
  );
}

function isUploadSummary(value: unknown): value is UploadSummary {
  return (
    isObject(value) &&
    isString(value.remote_user) &&
    isString(value.hostname) &&
    isString(value.remote_path) &&
    isNumber(value.files) &&
    isNumber(value.dirs) &&
    isNumber(value.bytes) &&
    isStringArray(value.new_files) &&
    isOptional(value.new_file_modes, isStringRecord) &&
    isOptional(value.creates_under, isString) &&
    isOptional(value.world_writable, isStringArray) &&
    isOptional(value.runs_code, isStringArray) &&
    isStringArray(value.missing_locally) &&
    isOptional(value.remote_conflicts, isConflicts) &&
    typeof value.ownership_may_be_incomplete === "boolean" &&
    isOptional(value.server_id_tail, isString)
  );
}

function isDifference(value: unknown): value is Difference {
  return (
    isObject(value) &&
    isString(value.remote_path) &&
    isString(value.change) &&
    typeof value.on_both_sides === "boolean"
  );
}

function isSkipped(value: unknown): value is SkippedEntry {
  return isObject(value) && isString(value.path) && isString(value.reason);
}

function isPathStatus(value: unknown): value is PathStatus {
  return (
    isObject(value) &&
    isString(value.host_key) &&
    isOptional(value.remote_path, isString) &&
    Array.isArray(value.sessions) &&
    value.sessions.every(isSession)
  );
}

/** Checks that `value` has the shape of a sync result, so that nothing else has to. */
export function parseSyncResult(value: unknown): SyncResult | undefined {
  if (!isObject(value)) {
    return undefined;
  }
  switch (value.status) {
    case "status":
      return isString(value.mirror_root) && isOptional(value.path, isPathStatus)
        ? (value as unknown as StatusResult)
        : undefined;
    case "downloaded":
      return isString(value.local_path) &&
        isNumber(value.files) &&
        isNumber(value.dirs) &&
        isNumber(value.bytes) &&
        isString(value.remote_user) &&
        Array.isArray(value.skipped) &&
        value.skipped.every(isSkipped) &&
        isOptional(value.baseline_warning, isString)
        ? (value as unknown as DownloadedResult)
        : undefined;
    case "needs_confirmation":
      if (!isString(value.pending_id)) {
        return undefined;
      }
      if (value.kind === "overwrite_local_changes" && isStringArray(value.files)) {
        return value as unknown as OverwriteConfirmation;
      }
      if (value.kind === "upload" && isUploadSummary(value.summary)) {
        return value as unknown as UploadConfirmation;
      }
      return undefined;
    case "uploaded":
      return isNumber(value.files) &&
        isNumber(value.dirs) &&
        isNumber(value.bytes) &&
        isString(value.remote_user) &&
        isOptional(value.backup_path, isString) &&
        isOptional(value.baseline_warning, isString)
        ? (value as unknown as UploadedResult)
        : undefined;
    case "compared":
      return Array.isArray(value.differences) &&
        value.differences.every(isDifference) &&
        isNumber(value.identical_files) &&
        isString(value.diff_path) &&
        isString(value.host_dir) &&
        isString(value.server_copy_dir) &&
        isString(value.remote_user)
        ? (value as unknown as ComparedResult)
        : undefined;
    case "unchanged":
      return isNumber(value.identical_files) ? (value as unknown as UnchangedResult) : undefined;
    case "cancelled":
      return value as unknown as CancelledResult;
    default:
      return undefined;
  }
}
