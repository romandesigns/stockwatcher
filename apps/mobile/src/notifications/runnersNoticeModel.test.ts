import { expect, test } from "bun:test";
import type { RunnersNoticeMessage } from "@stockwatcher/shared";

import { createPhoneNotices, noticeBannerData, phoneNotifyKinds } from "./runnersNoticeModel";
import { RUNNERS_PUSH_DEFAULTS } from "./runnersPushModel";

const notice = (over: Partial<RunnersNoticeMessage> = {}): RunnersNoticeMessage => ({ type: "runners.notice", id: "runners-burst-TNMG-1", alertId: "1", symbol: "TNMG", kind: "runners.burst", title: "Buying burst: TNMG $1.23", body: "Heavy buying", at: 1, ...over });

test("the phone's switches map onto the shared kinds one for one", () => {
  const kinds = phoneNotifyKinds({ ...RUNNERS_PUSH_DEFAULTS, pushBurst: false, pushHalt: true });
  expect(kinds.burst).toBe(false);
  expect(kinds.halt).toBe(true);
  expect(kinds.new).toBe(true);
  expect(Object.keys(kinds).length).toBe(Object.keys(RUNNERS_PUSH_DEFAULTS).length);
});

test("rings only with the server's flag, no push token and the kind's switch on, once per id", () => {
  let token: string | null = null;
  let prefs = RUNNERS_PUSH_DEFAULTS;
  const rung: RunnersNoticeMessage[] = [];
  const n = createPhoneNotices({ prefs: () => prefs, token: () => token, ring: (m) => rung.push(m) });
  // An older server: nothing (the card diffs instead), and no opt-in.
  expect(n.handle(notice())).toBe(false);
  expect(n.notifyMessage()).toBeNull();
  n.setServerSends(true);
  expect(n.notifyMessage()).toMatchObject({ type: "runners.notify", enabled: true, burst: true });
  expect(n.handle(notice())).toBe(true);
  expect(n.handle(notice())).toBe(false);
  prefs = { ...prefs, pushBurst: false };
  expect(n.handle(notice({ id: "runners-burst-TNMG-2" }))).toBe(false);
  token = "ExponentPushToken[x]";
  expect(n.notifyMessage()).toBeNull();
  expect(n.handle(notice({ id: "runners-new-TNMG-3", kind: "runners.new" }))).toBe(false);
  expect(rung.map((m) => m.id)).toEqual(["runners-burst-TNMG-1"]);
});

test("a rung banner carries the push's data, so a tap routes like the push", () => {
  expect(noticeBannerData(notice({ data: { symbol: "TNMG", kind: "runners.burst", alert_id: "1", extra: 2 } }))).toEqual({ symbol: "TNMG", kind: "runners.burst", alert_id: "1", event_id: "runners-burst-TNMG-1", extra: 2 });
});
