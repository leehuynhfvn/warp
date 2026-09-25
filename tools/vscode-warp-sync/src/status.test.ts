import assert from "node:assert/strict";
import { test } from "node:test";
import { describeFailure, describeStatus } from "./status";
import { SyncSession } from "./protocol";
import { WarpctrlError } from "./warpctrl";

function session(overrides: Partial<SyncSession> = {}): SyncSession {
  return {
    session_id: "s1",
    window_id: "w1",
    tab_index: 0,
    hostname: "draff3",
    user: "root",
    is_active: true,
    ...overrides,
  };
}

test("a session is shown as user@host", () => {
  const view = describeStatus({ host_key: "draff3", sessions: [session()] });

  assert.equal(view.text, "$(cloud) root@draff3");
  assert.equal(view.warning, false);
});

test("without a session the bar warns", () => {
  const view = describeStatus({ host_key: "draff3", sessions: [] });

  assert.equal(view.text, "$(warning) no Warp session");
  assert.equal(view.warning, true);
  assert.match(view.tooltip, /draff3/);
});

test("the active session is preferred and the others are counted", () => {
  const view = describeStatus({
    host_key: "draff3",
    sessions: [
      session({ user: "huynhl", is_active: false }),
      session({ user: "root", tab_index: 2 }),
    ],
  });

  assert.equal(view.text, "$(cloud) root@draff3 (+1)");
  assert.match(view.tooltip, /huynhl@draff3 — tab 1/);
  assert.match(view.tooltip, /root@draff3 — tab 3 \(active\)/);
});

test("a failure shows a warning with the reason", () => {
  const view = describeFailure(new WarpctrlError("Is Warp running?", "warp"));

  assert.equal(view.warning, true);
  assert.match(view.tooltip, /Is Warp running\?/);
});
