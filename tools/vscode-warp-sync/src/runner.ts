// Starts `warpctrl`. No shell is involved: the arguments are passed as they are.
import { execFile } from "node:child_process";
import { ProcessOutcome, RUN_TIMEOUT_MS } from "./warpctrl";

const MAX_OUTPUT_BYTES = 16 * 1024 * 1024;

export function runProcess(file: string, args: string[]): Promise<ProcessOutcome> {
  return new Promise((resolve) => {
    execFile(
      file,
      args,
      { timeout: RUN_TIMEOUT_MS, maxBuffer: MAX_OUTPUT_BYTES, windowsHide: true },
      (error, stdout, stderr) => {
        if (error === null) {
          resolve({ exitCode: 0, stdout, stderr });
          return;
        }
        const code = (error as NodeJS.ErrnoException).code;
        if (typeof code === "number") {
          resolve({ exitCode: code, stdout, stderr });
          return;
        }
        const failure =
          code === "ENOENT"
            ? `Could not run "${file}". Check the setting warpSync.command.`
            : `Could not run "${file}": ${error.message}`;
        resolve({ exitCode: null, stdout, stderr, failure });
      },
    );
  });
}
