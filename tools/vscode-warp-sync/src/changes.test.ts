import assert from "node:assert/strict";
import { test } from "node:test";
import { uploadableChanges } from "./changes";

const MODIFIED = 5;
const UNTRACKED = 7;
const INDEX_ADDED = 1;
const DELETED = 6;
const INDEX_DELETED = 2;
const IGNORED = 8;

test("modified, added and untracked files are uploadable", () => {
  const paths = uploadableChanges([
    { path: "/m/h/b", status: MODIFIED },
    { path: "/m/h/a", status: UNTRACKED },
    { path: "/m/h/c", status: INDEX_ADDED },
  ]);

  assert.deepEqual(paths, ["/m/h/a", "/m/h/b", "/m/h/c"]);
});

test("deleted and ignored files are left out", () => {
  const paths = uploadableChanges([
    { path: "/m/h/gone", status: DELETED },
    { path: "/m/h/staged-gone", status: INDEX_DELETED },
    { path: "/m/h/ignored", status: IGNORED },
    { path: "/m/h/kept", status: MODIFIED },
  ]);

  assert.deepEqual(paths, ["/m/h/kept"]);
});

test("a file that is both staged and modified appears once", () => {
  const paths = uploadableChanges([
    { path: "/m/h/a", status: INDEX_ADDED },
    { path: "/m/h/a", status: MODIFIED },
  ]);

  assert.deepEqual(paths, ["/m/h/a"]);
});
