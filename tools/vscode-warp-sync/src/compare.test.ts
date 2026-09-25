import assert from "node:assert/strict";
import { test } from "node:test";
import { MAX_DIFFS, planCompare } from "./compare";
import { ComparedResult, Difference } from "./protocol";

function compared(differences: Difference[]): ComparedResult {
  return {
    status: "compared",
    differences,
    identical_files: 0,
    diff_path: "/m/.warp-sync/diffs/h/etc.diff",
    host_dir: "/m/h",
    server_copy_dir: "/m/.warp-sync/compare/h",
    remote_user: "root",
  };
}

function both(path: string): Difference {
  return { remote_path: path, change: "changed_locally", on_both_sides: true };
}

function oneSided(path: string): Difference {
  return { remote_path: path, change: "new_on_server", on_both_sides: false };
}

test("files on both sides open as a diff with the server on the left", () => {
  const plan = planCompare(compared([both("/etc/nginx/nginx.conf")]));

  assert.deepEqual(plan, {
    diffs: [
      {
        left: "/m/.warp-sync/compare/h/etc/nginx/nginx.conf",
        right: "/m/h/etc/nginx/nginx.conf",
        title: "server ↔ mirror: /etc/nginx/nginx.conf",
      },
    ],
  });
});

test("a file that exists on one side only sends the report along", () => {
  const plan = planCompare(compared([both("/etc/a"), oneSided("/etc/b")]));

  assert.equal(plan.diffs.length, 1);
  assert.equal(plan.report, "/m/.warp-sync/diffs/h/etc.diff");
});

test("no more than the limit are opened, and the report covers the rest", () => {
  const files = Array.from({ length: MAX_DIFFS + 2 }, (_, index) => both(`/etc/f${index}`));

  const plan = planCompare(compared(files));

  assert.equal(plan.diffs.length, MAX_DIFFS);
  assert.ok(plan.report !== undefined);
});

test("exactly the limit needs no report", () => {
  const files = Array.from({ length: MAX_DIFFS }, (_, index) => both(`/etc/f${index}`));

  assert.equal(planCompare(compared(files)).report, undefined);
});

test("a path that would leave the mirror is not opened", () => {
  const plan = planCompare(compared([both("/../../etc/passwd"), both("/etc/ok")]));

  assert.deepEqual(
    plan.diffs.map((diff) => diff.title),
    ["server ↔ mirror: /etc/ok"],
  );
});
