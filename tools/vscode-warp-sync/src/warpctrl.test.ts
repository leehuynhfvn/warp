import assert from "node:assert/strict";
import { test } from "node:test";
import { ProcessOutcome, WarpctrlClient, WarpctrlError, buildInvocation, interpretOutcome } from "./warpctrl";

const CANCELLED = JSON.stringify({ status: "cancelled" });

function outcome(partial: Partial<ProcessOutcome>): ProcessOutcome {
  return { exitCode: 0, stdout: "", stderr: "", ...partial };
}

function failure(run: () => unknown): WarpctrlError {
  let thrown: unknown;
  try {
    run();
  } catch (error) {
    thrown = error;
  }
  assert.ok(thrown instanceof WarpctrlError, `expected a WarpctrlError, got ${String(thrown)}`);
  return thrown;
}

test("the operand goes after -- so that no file name can be read as an option", () => {
  const invocation = buildInvocation(["warpctrl"], "download", "-rf");

  assert.deepEqual(invocation, {
    file: "warpctrl",
    args: ["--output-format", "json", "sync", "download", "--", "-rf"],
  });
});

test("a command with its own arguments keeps them in front", () => {
  const invocation = buildInvocation(["/opt/warp-oss", "--warpctrl"], "status");

  assert.equal(invocation.file, "/opt/warp-oss");
  assert.deepEqual(invocation.args, ["--warpctrl", "--output-format", "json", "sync", "status"]);
});

test("an empty command is refused", () => {
  assert.equal(failure(() => buildInvocation([], "status")).kind, "usage");
  assert.equal(failure(() => buildInvocation([""], "status")).kind, "usage");
});

test("exit codes 0 and 3 both carry a result", () => {
  const pending = {
    status: "needs_confirmation",
    pending_id: "id-1",
    kind: "overwrite_local_changes",
    files: ["/etc/a"],
  };

  assert.equal(interpretOutcome(outcome({ stdout: CANCELLED })).status, "cancelled");
  const result = interpretOutcome(outcome({ exitCode: 3, stdout: JSON.stringify(pending) }));
  assert.equal(result.status, "needs_confirmation");
});

test("an error printed by Warp keeps its code and gets a hint when there is one", () => {
  const stdout = JSON.stringify({
    ok: false,
    error: { code: "local_control_disabled", message: "local control is disabled" },
  });

  const error = failure(() => interpretOutcome(outcome({ exitCode: 1, stdout })));

  assert.equal(error.kind, "warp");
  assert.equal(error.code, "local_control_disabled");
  assert.match(error.message, /local control is disabled/);
  assert.match(error.message, /Settings > Scripting/);
});

test("an error without a hint is passed on as it is", () => {
  const stdout = JSON.stringify({
    ok: false,
    error: { code: "stale_target", message: "That confirmation is no longer pending" },
  });

  const error = failure(() => interpretOutcome(outcome({ exitCode: 1, stdout })));

  assert.equal(error.message, "That confirmation is no longer pending");
});

test("a usage error from the command line is taken from stderr", () => {
  const error = failure(() =>
    interpretOutcome(outcome({ exitCode: 2, stderr: "error: unrecognized subcommand 'sync'\n" })),
  );

  assert.equal(error.kind, "usage");
  assert.match(error.message, /unrecognized subcommand/);
});

test("a process that could not be started is unavailable", () => {
  const error = failure(() =>
    interpretOutcome(outcome({ exitCode: null, failure: 'Could not run "warpctrl".' })),
  );

  assert.equal(error.kind, "unavailable");
});

test("output that is not a sync result is a protocol error", () => {
  assert.equal(failure(() => interpretOutcome(outcome({ stdout: "not json" }))).kind, "protocol");
  assert.equal(
    failure(() => interpretOutcome(outcome({ stdout: JSON.stringify({ status: "from_the_future" }) })))
      .kind,
    "protocol",
  );
});

test("the client runs the command from the current setting for every call", async () => {
  const calls: string[][] = [];
  let command = ["first"];
  const client = new WarpctrlClient(
    async (file, args) => {
      calls.push([file, ...args]);
      return outcome({ stdout: CANCELLED });
    },
    () => command,
  );

  await client.cancel("id-1");
  command = ["second", "--flag"];
  await client.prepareUpload("/m/h/etc");

  assert.deepEqual(calls, [
    ["first", "--output-format", "json", "sync", "cancel", "--", "id-1"],
    ["second", "--flag", "--output-format", "json", "sync", "upload", "--", "/m/h/etc"],
  ]);
});
