// Recognises the folders that are worth asking Warp about, so that no process is started for
// workspaces that have nothing to do with Warp Sync.
import * as path from "node:path";

function segments(folder: string): string[] {
  return path.resolve(folder).split(path.sep).filter((part) => part !== "");
}

/** Whether `folder` is inside Warp's default mirror folder (`~/.warp/mirrors`) or `mirrorRoot`. */
export function isMirrorCandidate(folder: string, mirrorRoot: string): boolean {
  const resolved = path.resolve(folder);
  if (mirrorRoot.trim() !== "") {
    const relative = path.relative(path.resolve(mirrorRoot), resolved);
    return relative !== "" && !relative.startsWith("..") && !path.isAbsolute(relative);
  }
  const parts = segments(resolved);
  const index = parts.indexOf(".warp");
  return index !== -1 && parts[index + 1] === "mirrors" && parts.length > index + 2;
}
