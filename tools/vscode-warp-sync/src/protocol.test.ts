import assert from "node:assert/strict";
import { test } from "node:test";
import { parseSyncResult } from "./protocol";

const summary = {
  remote_user: "root",
  hostname: "prod-1",
  remote_path: "/etc/nginx",
  files: 2,
  dirs: 1,
  bytes: 3000,
  new_files: [],
  missing_locally: [],
  ownership_may_be_incomplete: false,
};

test("every shape Warp produces is accepted", () => {
  const samples = [
    { status: "status", mirror_root: "/m" },
    {
      status: "status",
      mirror_root: "/m",
      path: {
        host_key: "prod-1",
        sessions: [
          { session_id: "s", window_id: "w", tab_index: 0, hostname: "prod-1", user: "root", is_active: true },
        ],
      },
    },
    {
      status: "downloaded",
      local_path: "/m/prod-1/etc",
      files: 1,
      dirs: 1,
      bytes: 1,
      remote_user: "root",
      skipped: [{ path: "/etc/l", reason: "symbolic link" }],
    },
    { status: "needs_confirmation", pending_id: "id", kind: "upload", summary },
    { status: "needs_confirmation", pending_id: "id", kind: "overwrite_local_changes", files: ["/a"] },
    { status: "uploaded", files: 1, dirs: 0, bytes: 5, remote_user: "root", backup_path: "/root/b.tgz" },
    {
      status: "compared",
      differences: [{ remote_path: "/etc/a", change: "changed_locally", on_both_sides: true }],
      identical_files: 3,
      diff_path: "/m/.warp-sync/diffs/h/etc.diff",
      host_dir: "/m/h",
      server_copy_dir: "/m/.warp-sync/compare/h",
      remote_user: "root",
    },
    { status: "unchanged", identical_files: 2 },
    { status: "cancelled" },
  ];

  for (const sample of samples) {
    assert.ok(parseSyncResult(sample) !== undefined, JSON.stringify(sample));
  }
});

test("a result with a missing or mistyped field is rejected", () => {
  const broken = [
    null,
    "text",
    [],
    {},
    { status: "status" },
    { status: "downloaded", local_path: "/m", files: "1" },
    { status: "needs_confirmation", kind: "upload", summary },
    { status: "needs_confirmation", pending_id: "id", kind: "upload", summary: { ...summary, files: "2" } },
    { status: "needs_confirmation", pending_id: "id", kind: "other" },
    { status: "compared", differences: [{ remote_path: "/a" }] },
    { status: "unchanged" },
    { status: "bogus" },
  ];

  for (const sample of broken) {
    assert.equal(parseSyncResult(sample), undefined, JSON.stringify(sample));
  }
});

test("null is not accepted where a field is optional", () => {
  const withNull = { status: "uploaded", files: 1, dirs: 0, bytes: 5, remote_user: "root", backup_path: null };

  assert.equal(parseSyncResult(withNull), undefined);
});
