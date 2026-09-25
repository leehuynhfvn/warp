// Runs `warpctrl sync ...` and turns what it prints into a typed result. Nothing here imports
// `vscode`, so it can be tested on its own.
import { parseSyncResult, SyncResult } from "./protocol";

/** The exit code of a sync command that changed nothing because it asks for a confirmation. */
const EXIT_NEEDS_CONFIRMATION = 3;

/** One sync operation runs several remote commands; `warpctrl` itself waits up to ten minutes. */
export const RUN_TIMEOUT_MS = 11 * 60 * 1000;

export type FailureKind = "unavailable" | "warp" | "usage" | "protocol";

export class WarpctrlError extends Error {
  constructor(
    message: string,
    readonly kind: FailureKind,
    readonly code?: string,
  ) {
    super(message);
    this.name = "WarpctrlError";
  }
}

export interface ProcessOutcome {
  /** Null when the process could not be started or was killed. */
  exitCode: number | null;
  stdout: string;
  stderr: string;
  /** Why the process could not be started or was killed. */
  failure?: string;
}

export type Runner = (file: string, args: string[]) => Promise<ProcessOutcome>;

export interface Invocation {
  file: string;
  args: string[];
}

/**
 * The process to start. `command` is `warpSync.command` (for example `["warpctrl"]`); the path or
 * id goes after `--` so that nothing a file name says can be read as an option.
 */
export function buildInvocation(
  command: readonly string[],
  subcommand: string,
  operand?: string,
): Invocation {
  const [file, ...prefix] = command;
  if (file === undefined || file === "") {
    throw new WarpctrlError("The setting warpSync.command is empty.", "usage");
  }
  const args = [...prefix, "--output-format", "json", "sync", subcommand];
  if (operand !== undefined) {
    args.push("--", operand);
  }
  return { file, args };
}

const HINTS: Record<string, string> = {
  local_control_disabled: "Turn on Settings > Scripting in Warp.",
  no_instance: "Is Warp running?",
  transport_unavailable: "Is Warp running, with Settings > Scripting turned on?",
  unauthorized_local_client: "Turn on Settings > Scripting in Warp.",
};

function withHint(message: string, code: string | undefined): string {
  const hint = code === undefined ? undefined : HINTS[code];
  return hint === undefined ? message : `${message} ${hint}`;
}

function parseJson(text: string): unknown {
  try {
    return JSON.parse(text);
  } catch {
    return undefined;
  }
}

function errorFromJson(value: unknown): WarpctrlError | undefined {
  if (typeof value !== "object" || value === null || (value as { ok?: unknown }).ok !== false) {
    return undefined;
  }
  const error = (value as { error?: { code?: unknown; message?: unknown } }).error;
  if (typeof error?.message !== "string") {
    return undefined;
  }
  const code = typeof error.code === "string" ? error.code : undefined;
  return new WarpctrlError(withHint(error.message, code), "warp", code);
}

/** Interprets what a finished `warpctrl sync` process printed. */
export function interpretOutcome(outcome: ProcessOutcome): SyncResult {
  if (outcome.failure !== undefined) {
    throw new WarpctrlError(outcome.failure, "unavailable");
  }
  const json = parseJson(outcome.stdout);
  if (outcome.exitCode === 0 || outcome.exitCode === EXIT_NEEDS_CONFIRMATION) {
    const result = parseSyncResult(json);
    if (result === undefined) {
      throw new WarpctrlError("Warp answered with something this extension does not understand.", "protocol");
    }
    return result;
  }
  const fromWarp = errorFromJson(json);
  if (fromWarp !== undefined) {
    throw fromWarp;
  }
  const detail = outcome.stderr.trim().split("\n").slice(-3).join(" ").trim();
  throw new WarpctrlError(
    detail === "" ? `warpctrl failed (exit code ${outcome.exitCode}).` : detail,
    "usage",
  );
}

export class WarpctrlClient {
  constructor(
    private readonly runner: Runner,
    private readonly command: () => readonly string[],
  ) {}

  private async run(subcommand: string, operand?: string): Promise<SyncResult> {
    const { file, args } = buildInvocation(this.command(), subcommand, operand);
    return interpretOutcome(await this.runner(file, args));
  }

  status(path?: string): Promise<SyncResult> {
    return this.run("status", path);
  }

  download(path: string): Promise<SyncResult> {
    return this.run("download", path);
  }

  /** Only prepares the upload: nothing is sent until `confirm`. */
  prepareUpload(path: string): Promise<SyncResult> {
    return this.run("upload", path);
  }

  compare(path: string): Promise<SyncResult> {
    return this.run("compare", path);
  }

  confirm(pendingId: string): Promise<SyncResult> {
    return this.run("confirm", pendingId);
  }

  cancel(pendingId: string): Promise<SyncResult> {
    return this.run("cancel", pendingId);
  }
}
