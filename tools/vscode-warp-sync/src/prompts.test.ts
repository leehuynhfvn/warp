import assert from "node:assert/strict";
import { test } from "node:test";
import { UploadSummary } from "./protocol";
import { count, formatSize, overwritePrompt, uploadPrompt } from "./prompts";

function summary(overrides: Partial<UploadSummary> = {}): UploadSummary {
  return {
    remote_user: "root",
    hostname: "prod-1",
    remote_path: "/etc/nginx",
    files: 2,
    dirs: 1,
    bytes: 3072,
    new_files: [],
    missing_locally: [],
    remote_conflicts: { changed: [], missing: [], already_exist: [] },
    ownership_may_be_incomplete: false,
    ...overrides,
  };
}

test("a clean upload names the user, the host and the amount, and offers a plain Upload", () => {
  const prompt = uploadPrompt(summary({ server_id_tail: "cdef" }));

  assert.equal(prompt.title, "Upload /etc/nginx to root@prod-1?");
  assert.equal(prompt.confirmLabel, "Upload");
  assert.equal(prompt.hasWarnings, false);
  assert.match(prompt.detail, /2 files and 1 folder \(3\.0 KiB\) will be written on root@prod-1 \(machine id ending cdef\), as root/);
  assert.match(prompt.detail, /backup/);
  assert.doesNotMatch(prompt.detail, /WARNING/);
});

test("conflicts with the server are spelled out and change the button", () => {
  const prompt = uploadPrompt(
    summary({
      remote_conflicts: {
        changed: ["/etc/nginx/nginx.conf"],
        missing: ["/etc/nginx/gone.conf"],
        already_exist: ["/etc/nginx/new.conf"],
      },
    }),
  );

  assert.equal(prompt.hasWarnings, true);
  assert.equal(prompt.confirmLabel, "Upload Anyway");
  assert.match(prompt.detail, /changed on the server since the last sync[^\n]*\n {4}\/etc\/nginx\/nginx\.conf/);
  assert.match(prompt.detail, /gone from the server/);
  assert.match(prompt.detail, /already on the server/);
});

test("a host that could not be checked is a warning too", () => {
  const prompt = uploadPrompt(summary({ remote_conflicts: undefined }));

  assert.equal(prompt.hasWarnings, true);
  assert.match(prompt.detail, /could not be checked/);
});

test("new and missing files and a partial ownership note are listed", () => {
  const prompt = uploadPrompt(
    summary({
      new_files: ["/etc/nginx/new.conf"],
      missing_locally: ["/etc/nginx/old.conf"],
      ownership_may_be_incomplete: true,
    }),
  );

  assert.match(prompt.detail, /New files:\n {4}\/etc\/nginx\/new\.conf/);
  assert.match(prompt.detail, /will not be deleted on the server\):\n {4}\/etc\/nginx\/old\.conf/);
  assert.match(prompt.detail, /not GNU tar/);
});

test("the mode each new file gets is shown", () => {
  const prompt = uploadPrompt(
    summary({
      new_files: ["/etc/nginx/secret.conf", "/etc/nginx/plain.conf"],
      new_file_modes: { "/etc/nginx/secret.conf": "0600" },
    }),
  );

  assert.match(prompt.detail, /New files:\n {4}\/etc\/nginx\/secret\.conf \(mode 0600\)\n {4}\/etc\/nginx\/plain\.conf$/);
});

test("a path that is not on the server yet says what is created, where, and that nothing is replaced", () => {
  const prompt = uploadPrompt(
    summary({
      remote_path: "/root/test-dir-2",
      new_files: ["/root/test-dir-2", "/root/test-dir-2/a.conf"],
      new_file_modes: { "/root/test-dir-2": "0755", "/root/test-dir-2/a.conf": "0644" },
      creates_under: "/root",
    }),
  );

  assert.match(prompt.detail, /Creates on the server, inside \/root:\n {4}\/root\/test-dir-2 \(mode 0755\)\n {4}\/root\/test-dir-2\/a\.conf \(mode 0644\)/);
  assert.match(prompt.detail, /Nothing on the server is replaced, so no backup is made\./);
  assert.doesNotMatch(prompt.detail, /A backup of what is replaced/);
  assert.doesNotMatch(prompt.detail, /New files:/);
  assert.equal(prompt.hasWarnings, false);
  assert.equal(prompt.confirmLabel, "Upload");
});

test("long lists stop after ten paths", () => {
  const files = Array.from({ length: 13 }, (_, index) => `/etc/f${index}`);

  const prompt = overwritePrompt(files);

  assert.match(prompt.detail, /13 files/);
  assert.match(prompt.detail, /\/etc\/f9/);
  assert.doesNotMatch(prompt.detail, /\/etc\/f10/);
  assert.match(prompt.detail, /and 3 more/);
  assert.equal(prompt.confirmLabel, "Overwrite");
});

test("counts and sizes read naturally", () => {
  assert.equal(count(1, "file"), "1 file");
  assert.equal(count(0, "file"), "0 files");
  assert.equal(formatSize(512), "512 B");
  assert.equal(formatSize(1536), "1.5 KiB");
  assert.equal(formatSize(5 * 1024 * 1024), "5.0 MiB");
});
