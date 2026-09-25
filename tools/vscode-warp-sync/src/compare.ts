// Decides which files a comparison opens in VS Code's diff editor.
import * as path from "node:path";
import { ComparedResult } from "./protocol";

/** More tabs than this would bury the window. Warp uses the same limit. */
export const MAX_DIFFS = 10;

export interface DiffPair {
  /** The server's copy. */
  left: string;
  /** The file in the mirror. */
  right: string;
  title: string;
}

export interface ComparePlan {
  diffs: DiffPair[];
  /** The written comparison, opened when some differences cannot be shown side by side. */
  report?: string;
}

function inside(root: string, candidate: string): boolean {
  const relative = path.relative(root, candidate);
  return relative !== "" && !relative.startsWith("..") && !path.isAbsolute(relative);
}

/** `remotePath` under `root`, or undefined when it would leave `root`. */
function under(root: string, remotePath: string): string | undefined {
  const resolved = path.resolve(root, ...remotePath.split("/").filter((part) => part !== ""));
  return inside(root, resolved) ? resolved : undefined;
}

export function planCompare(result: ComparedResult): ComparePlan {
  const both = result.differences.filter((difference) => difference.on_both_sides);
  const oneSided = result.differences.length - both.length;
  const diffs: DiffPair[] = [];
  for (const difference of both.slice(0, MAX_DIFFS)) {
    const left = under(result.server_copy_dir, difference.remote_path);
    const right = under(result.host_dir, difference.remote_path);
    if (left !== undefined && right !== undefined) {
      diffs.push({ left, right, title: `server ↔ mirror: ${difference.remote_path}` });
    }
  }
  const showsEverything = oneSided === 0 && both.length <= MAX_DIFFS;
  return showsEverything ? { diffs } : { diffs, report: result.diff_path };
}
