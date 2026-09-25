import assert from "node:assert/strict";
import { test } from "node:test";
import { isMirrorCandidate } from "./mirror";

test("folders below ~/.warp/mirrors are candidates", () => {
  assert.equal(isMirrorCandidate("/home/u/.warp/mirrors/prod-1", ""), true);
  assert.equal(isMirrorCandidate("/home/u/.warp/mirrors/prod-1/etc/nginx", ""), true);
});

test("the mirror root itself and unrelated folders are not", () => {
  assert.equal(isMirrorCandidate("/home/u/.warp/mirrors", ""), false);
  assert.equal(isMirrorCandidate("/home/u/.warp", ""), false);
  assert.equal(isMirrorCandidate("/home/u/projects/app", ""), false);
  assert.equal(isMirrorCandidate("/home/u/mirrors/prod-1", ""), false);
});

test("a configured mirror root replaces the default", () => {
  assert.equal(isMirrorCandidate("/data/sync/prod-1", "/data/sync"), true);
  assert.equal(isMirrorCandidate("/data/sync", "/data/sync"), false);
  assert.equal(isMirrorCandidate("/data/syncing/prod-1", "/data/sync"), false);
  assert.equal(isMirrorCandidate("/home/u/.warp/mirrors/prod-1", "/data/sync"), false);
});
