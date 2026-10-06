// A teammate's idle notification that repeats its report, against the engine.
import { expect, test } from "claude-code/testing";

const REPORT = "The measure area is done. Built 79 files under app/Quality, every battery passes its checks, " +
  "the release gate reads the new thresholds, and the dashboard shows the results per framework with their evidence.";

const idle = (from, result) => JSON.stringify({ type: "idle_notification", from, timestamp: "2026-09-30T18:00:00Z", idleReason: "available", result });
/** The engine beneath: it queues what it is given. */
const queue = (on) => on("session.receive", (_$, e) => ({ text: e.text }));
const from = (teammate) => ({ kind: "peer", teammate, isVerified: true });

test("an idle notification that repeats the report gets a short result", async ($, on) => {
  queue(on);
  await $.session.receive({ origin: from("judge-agent"), text: REPORT });
  const { text } = await $.session.receive({ origin: from("judge-agent"), text: idle("judge-agent", `Done. ${REPORT}`) });
  const value = JSON.parse(text);
  expect(value.type).toBe("idle_notification");
  expect(value.result).toContain("sessionkit: left out, it repeats the message judge-agent sent at");
});

test("a new result, or one from another teammate, stays", async ($, on) => {
  queue(on);
  await $.session.receive({ origin: from("judge-agent"), text: REPORT });
  const other = await $.session.receive({ origin: from("pages-agent"), text: idle("pages-agent", REPORT) });
  expect(JSON.parse(other.text).result).toBe(REPORT);
  const fresh = "The twelve pages are built and checked in the browser on both themes; the translation keys are complete for Dutch and English, and no route returns an error.";
  const own = await $.session.receive({ origin: from("judge-agent"), text: idle("judge-agent", fresh) });
  expect(JSON.parse(own.text).result).toBe(fresh);
});

test("a message from outside the team passes as it is", async ($, on) => {
  queue(on);
  const { text } = await $.session.receive({ origin: { kind: "peer" }, text: idle("x", REPORT) });
  expect(JSON.parse(text).result).toBe(REPORT);
});
