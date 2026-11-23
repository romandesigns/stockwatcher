# The chart packages: what they hold and how to embed them

Status: current as of 2026-10-18. This is the starting point for any new screen that shows market data.
The history of how the chart was built is in `simulator-and-chart.md`; this file is the map.

## The dashboard (the main page, 2026-10-18)

The reference for a screen with several charts. Web `apps/web/src/dashboard/`, phone
`apps/mobile/src/dashboard/` + `screens/DashboardScreen.tsx`.
- **Panel of 1–4 charts**, each a full `CandleChart` in its own container on its own feed slot.
  The "screen × n" control (`MultiViewMenu` / `CountSheet`) sets n. Web lays the panel out by
  the panel's own width (container queries: stacked below 48rem, two columns from 48rem, three
  across from 72rem for three charts). The phone caps the synced count by its screen
  (`dashboard/layout.ts` `maxChartsFor`, Android's sw600dp rule): 1 chart on a phone-size screen
  (the owner's Galaxy Z Fold folded), up to 2 unfolded, stacked while the panel is taller than
  wide; the cap is never written back. It fills the space between header and list, so no
  scrolling fights the chart's pan gestures.
- **One header row per web cell** (owner, 2026-10-18): `CandleChart` `headingInToolbar` puts the
  ticker, price and change at the toolbar's left and the timeframe with the tools on its right,
  instead of floating in the chart's corners. With more than one chart, `timeframeControl="wheel"`
  swaps the four segments for `TimeframeWheel`, the phone's scroll wheel on the web (mouse wheel or
  trackpad over it, drag, arrow keys; up is the longer timeframe).
- **Select a chart** by pressing it: an accent ring drawn as a box shadow (no border). A picked
  ticker (the header's ticker search on the web; the search or the live "Most active" list on the
  phone) loads into the selected chart. Clicking outside every chart hides the ring (web); the
  selection stays. What each chart shows syncs (app-state `dashboard`); the selection is per device.
- **Indicators and levels, linked or per chart** (owner, 2026-10-18): Settings → Chart → Visibility
  → "Same indicators and levels on every chart" (`ChartVisibility.linkStudies`, default on, synced).
  On, every chart reads and edits the shared `indicators` / `layers`; off, each dashboard chart has
  its own on its slot (app-state `DashboardSlot.indicators` / `layers`, null = follows the shared
  set), kept while linked so unlinking brings it back. The chart page always uses the shared set.
  The chart packages never read the flag: the app picks the set per chart (`ChartCell` / `ChartTile`).
- **Copy a ticker**: tap, then press and hold on any ticker label (chart headings, list rows) copies
  it, with a "Copied …" toast that fades out. One recognizer, `@stockwatcher/shared` `tapHold.ts`;
  a plain tap waits out the 300 ms double-tap window before acting. Web `lib/useTickerPress.ts` +
  `lib/toast.tsx`; phone `ui/useTickerPress.ts` (expo-clipboard) + `ui/Toaster.tsx`.
- **Halt watcher** (owner, 2026-10-19; design canvas "Halt Watcher",
  https://claude.ai/artifact/R5VpkCELmhv4srLeuEuv7b; reworked 2026-10-20, owner: "observe for a
  rapid increasing amounting pressure that could potentially lead to a halt", and no card when
  none is happening): the symbols whose one-sided pressure is building up fast while the price
  runs at its LULD band, at most 6, usually none ("No build-ups right now"), as gauge cards (a
  radial half ring filled with the halt risk; ticker, price and change right; a cue under the
  ticker such as "+45 in 30s · 1.2% to LULD up"). Direction is the hue (green toward limit up,
  red toward limit down);
  a halted card keeps its last risk on a grey arc and shows "Halted ~HH:MM" in amber (a LULD
  pause's expected reopening; another halt shows when it began). Pressing a card loads its ticker
  into the selected chart. Web: a strip above the charts inside the panel's scroll (`HaltWatcher`,
  three columns wide, two narrower, one on a phone, by container queries). Phone: the region under
  the charts, a `Segmented` "Halt watcher | Most active" (one column folded, two unfolded by
  `maxChartsFor`'s sw600dp rule). Data: `pressure.subscribe` on the dashboard's own socket, so it
  reads the live market like the charts (the practice page reads its own replay connection). The gauge is the shared `GAUGE` geometry and colours in
  `@stockwatcher/shared` `pressure.ts`, drawn by `PressureGauge` on each platform, and is wavykit's
  `pressure-gauge`.
  - **Pressure** (server, `crates/server/src/pressure.rs`, trades only: the feed has no quotes):
    over the last 60 s of OHLC-counted prints (the engine's verdict, so the candles' condition
    policy), sides by the tick rule (uptick buy, downtick sell, zero tick inherits).
    `buyShare = B / (B + S)`; side bull when ≥ 0.5. `I = min(1, |2·buyShare − 1|)` (100/0 is
    fully one-sided). `pace = V / A`, `A` the symbol's average volume per minute so far in the
    session (premarket, regular, after hours each on its own; at least one minute).
    `P = min(1, ln(1 + pace) / ln 101)`. **`pressure = round(100 · I · (0.4 + 0.6 · P))`**. No
    score below 10 prints or 2,000 shares in the window. Frozen at a halt until the resume.
  - **Halt risk** (what the gauge shows; formula, constants and their reasons in the module docs):
    rise `Δ` = the side's pressure now minus 30 s earlier; distance `d` to the modelled LULD band
    on that side and the band's half-width `h`; move `m` toward it over 30 s; time to band
    `T = d · 30 / m`. `risk = round(100 · (0.4 · min(1, Δ/50) + 0.35 · U(T) + 0.25 · (1 − d/h)))`,
    `U` logarithmic from 1 at 15 s to 0 at 10 min. Regular session only (no band otherwise).
  - **Entry / exit**: in the server's price band, with a band; enters on `Δ ≥ 30`, a move toward
    the band of 5 % of its half-width, past half-way to it, `T ≤ 180 s` and risk ≥ 50; stays while
    building (`Δ > 0` and not moving away, or pinned within 10 % of the half-width of the band),
    leaves 5 s after it stops; a halted card that was on the board or building at the halt stays
    through it, frozen. At most 6, strongest first; an outsider replaces the weakest unhalted
    member only by 10 points, and a card moves above its neighbour only by 10 points. Ranked once
    per second (the quotes task, off the ingest path); one pre-serialized `pressure.update` with
    the whole board goes to subscribers when it changed. Calibration bench:
    `crates/server/tests/halt_bench.rs` (ignored; `HALT_BENCH_ARCHIVE=<archive root>`).

## Alerts (2026-10-19)

Price alerts, built to the owner-approved design ("StockWatcher alerts", Strategy board = the
spec). Rules and storage: `app-state.md` "Alerts"; evaluation: `crates/server/src/alerts`.

**What you can watch.** Price crosses above/below a level; a % move up/down/either within 1, 5 or
15 min, since today's open or since yesterday's close; signals: halted or resumed, within N% of
the limit-up/down band (**modelled**, chart-engine `luld.rs`; every text says so), new high of
day, Halt watcher pressure reaching N (`crate::pressure`, scored once a second only for the
symbols such an alert watches). Fires once (→ done) or every time with a 5–60 min gap (default
15); sessions pre / regular / after / overnight; expires end of day (20:00 ET) / end of week
(Friday 20:00 ET) / never; notify in app and/or phone.

**Evaluation (server, live market only, off the ingest path).** `Market::apply` offers each
eligible print (`Trade::effect().updates_ohlc`, the candles' own rule), halt, resume, corrected
print and modelled LULD change to the alerts task with `try_send` on a bounded channel (8192);
a full channel drops and counts (`alerts.status.dropped`). The rules (`eval.rs`, each pinned by a
test): a cross needs the previous eligible trade on the other side (a touch is not a cross); a
comparison against a previous trade older than 10 min (regular) / 30 min (outside), or a print
that much older than the wall clock, is held back as **stale**; sessions filter silently; while
halted only halt alerts fire (others are held, `halted`); "every time" re-arms only when the gap
has passed AND the price moved back 0.5% from the fire price (halt and pressure alerts: the gap
alone); a met condition while cooling is held once (`gap`); a late print (> 2 s older than the
newest) or a correction's replacement fires a price alert once, marked late, without moving the
reference; % moves, the LULD distance and pressure are edges (false → true); rules loaded at
startup learn before they fire, so a restart never re-fires what is currently true.

**Delivery.** In app always: `alert.fired` to every socket, a toast, an Activity row. Phone through
Expo's push service, in order: the alert's Phone chip, a registered phone, quiet hours (ET),
one per stock per 5 min (extra fires grouped into the next: "+2 more"), the daily budget (5/10/20/50,
default 20, restored from Activity after a restart). Every hold-back is in Activity with its reason;
Activity is kept 30 days (`DATA_DIR/alerts/history.jsonl`). Expo is a hosted dependency on
purpose (a locked phone is reachable only through FCM/APNs): optional at runtime, batched (100 per
request), retried on 429/5xx, receipts checked after 15 min, `DeviceNotRegistered` drops the
token.

**Runners pushes (2026-10-22).** The same task and the same Expo sender also push the live
Runners board's new flags and, per phone, its breakouts (`crates/server/src/alerts/runners.rs`):
the market offers each notice with `try_send` after the core lock, a replay never does. Opt-ins
live on the device record (`push.register {runnersNew, runnersBreakout}`; new flags on, breakouts
off), not in the synced settings: they are about which phone makes noise. Limits of their own:
one per stock per 10 min, 30 a day (kept across restarts, apart from the price alerts' budget),
the price alerts' quiet hours, nothing older than 3 min. Not in Activity (logged only). A tap
opens the ticker's chart (`apps/mobile/src/notifications/route.ts`); while the phone has no push
token, the dashboard's local banner for new flags stands in.

**On the chart (chart-core `alerts.ts`, both renderers; the chart never talks to the server, the
app passes `alerts={lines, onCreate, onMore, onMove, onDelete, onUndo}`).**

| To… | Web (mouse) | Phone (touch) | Keyboard |
|---|---|---|---|
| New alert at a price | the bell beside the hover "+" (same y); press and hold 0.5 s anywhere in the plot (a ring appears after 150 ms; moving > 4 px first is a pan; never with a drawing tool armed) → the quick card at the exact price (the chart's own scale, linear/log/percent, snapped to $0.0001 under $1, $0.01 above; above the last price = crosses above) | a long press still scrubs; on lift a 4 s chip "Alert / + Trade"; long-press the price axis → "Alert at X"; the quick sheet with −/+ one tick | Alt+A at the crosshair (the last price without one) |
| Move | drag its line or tag (Shift = a tenth as fast), "old → new · %" live; Esc puts it back; release saves with an Undo toast | drag its 44 pt axis handle (no long press), a haptic tick per price step; lift saves with Undo | Tab to the tag, ↑/↓ one tick, Enter saves, Esc cancels |
| Delete | drag the tag off the right edge ("Release to delete") | drag the handle off the right edge | Delete on a focused tag |
| Undo | the toast's Undo, or Ctrl+Z (the chart's Ctrl+Z when its last change was an alert's) | the toast's Undo (5 s) | |

Grab priority: orders and positions, then alerts, then drawings (`hitObject`). A moved line's
direction follows the last price; the edit re-arms it (a moved finished alert fires again).
GestureController: `holdAlert` / `liftChip` context flags, `holdRing` / `holdAlert` / `placeAlert` /
`liftChip` actions, `objectDrag.fine` / `.cancelled`, `cancelDrag()`.

**Pages.** Web: the Alerts rail item (bell, badge = fires since the page was last open on this
device) → the manager (state chips: Armed / Cooling · 9 min / Snoozed / Fired 10:42 / Expired;
row menu: Edit, Snooze, Re-arm, Delete) and Activity by day, the phone budget and quiet hours.
Phone: the third tab (bell, the same tab-switch spec), the list, Activity, "Turn on phone alerts",
a banner on each fire.

## Drawing alerts (2026-11-09)

Owner request: "If the price line merges with the drawing line, I want some visual cue to indicate
exactly that and even an alert sound I can manage for this behavior. I want to use this strategy
whenever trading after regular market hours where stop loss is disabled. Take into consideration
the other drawing shapes and reproduce the behavior for them." A drawing alert is a price alert
whose level is one of your drawings, evaluated on the server (so it works with the app closed and
in extended hours) by the same evaluator, with the same rules.

**What can be watched (v1).** A horizontal line; a trend line (active only between its two
anchors); a ray (from anchor a, through b, and on into candles not printed yet; a ray pointing
back in time never reaches a new candle); a long or short position's entry, target and stop, each
its own alert (the stop is the after-hours mental stop the feature exists for, so the form offers
it first). Rectangles, supply/demand zones and vertical lines are the second pass (below).

**Trigger: touch with tolerance.** An eligible trade (the price alerts' rule:
`Trade::effect().updates_ohlc`) within the tolerance of the drawing's level at that trade's candle:
`|price - level(t)| <= tol`. The tolerance is whole ticks of the level's grid ($0.0001 under $1,
$0.01 at and above; default 1, 0 = exactly, at most 100) or a % of the level (at most 10). A
print that jumped clean through the level since the previous eligible print also counts ("Crossed
through"): an illiquid after-hours stock can skip a one-tick band entirely, and a skipped stop is
exactly the one that must not stay silent. It is an edge like the other level conditions (fires
when it becomes met), so a price sitting on the line fires once, not on every print.

**Same rules as price alerts** (`crates/server/src/alerts/eval.rs`, each pinned by a test):
sessions filter silently (a new drawing alert gets the sessions picked last plus After; Overnight
is a chip away); while halted it is held (`halted`); a stale comparison is held (`stale`); once →
done; every time → cooling for the gap, and re-armed once the price has moved 0.5% away from where
it fired, **either way** (a touch has no side to come back from; this is the one deliberate
difference); the phone budget, the per-stock 5 min limit and quiet hours apply to its push;
expiry and snooze as for any alert. A new drawing alert never expires by default: a stop that
silently expired is worse than one that was never set. Late and corrected prints do not fire it
(only price alerts take late prints).

**The geometry lives in the drawings, not in the alert.** The rule names the drawing
(`drawingId`) and the level (`part`); the server reads the drawing from the synced
`drawings:<SYMBOL>` namespace (the same document the charts edit) and follows every change:

- **Edited** (dragged, re-angled, a position's stop moved): the alert follows the new geometry and,
  like dragging a price alert's line, a finished or cooling alert starts over armed. The new level
  is learned before it fires, so a line dropped right onto the price does not fire on the next
  print; it fires when the price comes back to it.
- **Deleted**: the alert stays, orphaned (`alerts.state` entry `orphaned: true`, "Drawing deleted"
  on every client), and watches nothing. An undo that brings the drawing back resumes it. Deleting
  the alert is the person's call.

**Sloped lines and the slot axis (the approximation, stated).** The chart's x axis is a slot
index (`scale.ts`): every candle and every gap run takes one slot, however long the gap, and the
time between sessions takes none. A trend line or ray is straight in slots, so its price at a
candle is interpolated in slots, never on the clock: `level(t) = a.p + (b.p - a.p) × (i - ia) /
(ib - ia)` with `i`, `ia`, `ib` the fractional slot indexes (`tToIndex`, including its off-the-end
rule of one slot per timeframe interval). The level at a trade is the level at the centre of the
trade's candle, which is where the person sees the price meet the line. The slots depend on the
timeframe, so the alert records the chart it was set on (`tf`, the chart's timeframe when the bell
was pressed) and the server builds that timeframe's slots from the engine's candles and gap runs
(`Market::alert_slots`, chart-core `buildSlots` ported; extended hours shown), once per new candle.
Exact while both anchors lie within the engine's retained sessions (four per timeframe); an older
anchor is placed by the off-the-end rule, which is also what a chart without that history loaded
does, but a chart with older history loaded can draw that line slightly differently. With extended
hours hidden, the chart's own slots skip the extended sessions and the line is drawn differently
from what the server evaluates; the after-hours use this exists for has them shown. Horizontal
lines and position levels are flat and exact.

One rule, two languages: `crates/server/src/alerts/geometry.rs` (`level_at`, `t_to_index`,
`build_slots`, `Tolerance`) and `packages/chart-core/src/drawingAlerts.ts` (`drawingLevelAt`,
`tToIndex`, `parseDrawingTolerance`) read the same golden cases,
`crates/server/src/alerts/drawing_alert_fixtures.json` (trend across a gap, ray past b and into the
future, reversed and vertical lines, position parts, an anchor older than the slots).

**The cue (chart-core `drawingAlerts.ts`, one display list for both renderers).** The host passes
`drawingAlerts={watched, onAlert}`; the chart computes everything against its own drawings and
slots.

- **Touching:** a live (armed) drawing alert whose level the last price is within the tolerance of
  glows: the level's segment drawn again wide and translucent, breathing on a 1.6 s cycle (a steady
  glow under reduced motion), with an axis tag at the level and a "touching" pill beside it
  ("touching stop" for a stop). Accent; a stop uses the down hue (a status colour that means
  something). The renderers tick the clock (20 fps) only while something is touching or flashing.
- **Fired:** a flash that expands and fades (1.4 s, none under reduced motion) and a ring on the
  candle where it fired, kept until the person acknowledges it (the toast's or banner's "Got it",
  or "Acknowledge fire" on the Alerts page/tab). Unacknowledged fires are session state
  (`drawingFires`), not synced: another device's or an old fire is in Activity, not re-flashed.
- **Watched, not touching:** a small bell at the middle of the level. The touching pill sits at the middle too: beside the axis it covered the newest candles, at the left end the drawing toolbox hid it (both seen on screen).
- **The toolbox bell** (an action like Delete, hideable in the Drawing tab) is enabled on a selected
  line, ray or position and opens the app's alert form for it (an existing alert opens for
  editing). The form: the level (a position's stop / target / entry), the tolerance in ticks or %,
  once / every, sessions, expiry, In app / Phone / Sound.

**Sound.** One tone per fire event, distinct by how it was met: up (the price came up into the
drawing: a rising two-note), down (falling two-note), stop (a position's stop, either way: a low
triple). The score is `ALERT_TONES` in app-state, used by both clients.

- Which fires sound: app-state `alertSoundFor`: a drawing fire (it carries a `tone`), the alert's
  own Sound chip, the global switch (`alerts.prefs.sound.on`, synced: mute once, muted everywhere),
  outside quiet hours.
- Web: generated with WebAudio (no audio files), at the global volume (`alerts.prefs.sound.volume`,
  a perceptual curve). Browsers keep audio locked until the page has had a press or a key; the
  first one anywhere unlocks it (and turning sounds on or a Test button does); a fire while still
  locked says so in its toast. Settings: the speaker button on the Alerts page (switch, volume,
  a test of each tone).
- Phone (Android): three notification channels, `drawing-up-v1`, `drawing-down-v1`,
  `drawing-stop-v1`, each with its own bundled WAV (`apps/mobile/assets/sounds`, rendered by
  `scripts/tones.ts` from the same score; a test pins the files to it). A channel's sound is fixed
  when it is created, so the tone is the channel, and the `-v1` suffix is the tone's version: a new
  tone needs new ids on both sides. A fire heard in the open app plays the tone as a local
  notification on that channel when no push brought it to this phone. Volume is the phone's
  notification volume (the volume slider is the web's); the budget sheet has the switch and test
  buttons.
- **Old binaries are never given a tone channel.** The JS can reach a phone before the APK that
  bundles the WAVs (an over-the-air update), and a channel created without its sound resource would
  keep the wrong sound for good. So `apps/mobile/src/notifications/toneSupport.ts` gates every tone
  path on the installed binary's native app version (expo-application's
  `nativeApplicationVersion`, Android versionName; an OTA update cannot change it): the APK that
  adds the sounds is version 0.2.0 (`app.config.ts`), and below it no drawing channel is created, no
  tone is played, and the budget sheet says to update the app. The phone reports the result in
  `push.register` (`drawingTones: true|false`); the server sends a sounding drawing alert's push on
  the tone channel only to phones that reported true (`push::Device::drawing_tones`, absent in old
  records = false) and on the plain `alerts` channel to every other phone, including a muted alert
  (chip or global switch). The fingerprint runtime version already keeps this JS off older
  binaries (the plugin's `sounds` are part of the fingerprint); the version gate holds even if that
  policy changes.

**Second pass (designed, not built).** Each is a new `part` or `kind` on the same rule and the same
evaluator, with its geometry in `geometry.rs` and `drawingAlerts.ts` against the same fixtures:

- **Range (rect):** active only between its two anchors' times (slot space, like a trend line).
  `part: "enter"` fires when an eligible trade lands inside the box (between its two prices, with
  the tolerance) after one outside it; `part: "break"` fires on the first trade beyond the top or
  bottom edge after one inside, its tone up or down by the edge. A turned range (`angle`) is a
  screen angle with no data meaning (drawings.ts), so the alert uses the unturned box and the form
  says so.
- **Supply / demand zone:** open to the right from its earlier anchor, so always active once
  started. `enter`: a trade inside the band after one outside (supply from below, demand from
  above, the side it is approached from). `break`: a CANDLE CLOSE beyond the far edge (above
  supply, below demand) on the alert's timeframe, decided when the engine finalizes the bar, not on
  a print (a wick through a zone is the zone working). Tones: enter = toward, break = the stop tone.
- **Vertical line:** a time alarm, not a price condition: fires once when the market clock reaches
  its time (the slot's time on the alert's timeframe; a time inside a gap run fires at the first
  print or the clock reaching it), in the alert's sessions, with no price. Re-arming makes no
  sense for it, so it is Once only.

**Wire.** The rule (`alerts` namespace, per entry):
`{…the price alert fields…, rule: {kind: "drawing", drawingId, part: "line"|"entry"|"target"|"stop",
tol: {unit: "ticks"|"pct", value}, tf: "15s"|"30s"|"1m"|"5m"}, sound: bool}` (`sound` defaults to
true; older clients ignore the unknown kind and skip the rule, as their parsers do). `alerts.state`
entries may carry `orphaned: true`. `alerts.prefs` gains `sound: {on, volume}`. An Activity event
(`alert.fired`, `alerts.history`) of a drawing alert carries `tone: "up"|"down"|"stop"`; its push's
data carries `tone` and goes out on the tone's channel (phones with `drawingTones` only).
`push.register` gains `drawingTones?: boolean` (absent = unchanged; a new phone without it = false).

## Halt sounds (2026-11-16, release 1)

Owner-approved plan "halt sounds": two notifications per stock, **Halted** and **Resumed**, from the
exchange's own trading-status feed (not the Halt watcher, which is unchanged), with a sound per event
chosen on each device.

- **Server** (`crates/server/src/alerts/halts.rs`): covered stocks only (today's Runners board, a
  symbol an open live client has on a chart, or one with a per-stock halt rule). The events take
  the Runners notices' delivery and limits (`alerts/runners.rs`): dedupe by symbol and halt start,
  persisted per day; the devices that want the kind; quiet hours (held, logged in the notice
  ledger and the Insights log); the 3-minute stale drop; no daily cap. Resumed waits up to 15 s
  for the reopening print so its "change from the halt price" is real. Text: "SYM · Halted ·
  $2.76" / "LULD pause · +42.3% on the day"; "SYM · Resumed · $3.10" / "Halted 5 min · +12.3% from
  $2.76". A tap or click opens the chart. Per-stock halt rules (`Condition::Halt`) still fire as
  before; a stock with one gets both.
- **Sounds** (`packages/app-state` `HALT_SOUNDS`): eight synthesized scores (siren, klaxon, triple
  beep, alarm, chime, ding, rise, blip) and None. Defaults: Siren for Halted, Chime for Resumed.
  The web and desktop play them with WebAudio (`apps/web/src/alerts/sound.ts` `playNotes`) at the
  alert sounds' synced volume and mute; `apps/mobile/scripts/tones.ts` renders the same scores to
  `assets/sounds/halt_<key>.wav` (10–36 KB) for release 2.
- **Settings → Alerts → Halts** (web and desktop, `apps/web/src/notify/HaltRows.tsx`): per event a
  switch, a sound picker, ▶ preview and "Send test" (`halts.test`: the server answers that
  connection with a real `runners.notice` for the stock TEST). Choices are per device (the
  notifier's localStorage prefs). Halts work without system notifications: the connection opts in
  to the two kinds, plays the sound and shows an in-app toast; a page that has not had its click
  says so. The phone gets the two switches (shared `ALERT_SWITCH_SECTIONS`, over the air).
- **Wire.** `runners.notice` kinds `halt.halted` / `halt.resumed`, `data.sound` the event's default
  key (a phone push carries the phone's own); `runners.notify` and `push.register` gain `halted?`,
  `resumed?`; `push.register` also `haltedSound?`, `resumedSound?` and the capability
  `haltSounds?`; client `halts.test {event}`. Older clients drop the unknown kinds
  (`parseRunnersNotice`), and an older phone shows the push as usual (default sound, `alerts`
  channel; a tap with a symbol opens its chart).
- **Release 2 (phone sounds, needs a new APK):** list the eight WAVs in `app.config.ts`'s
  expo-notifications `sounds`, create one Android channel per sound (`halt-<key>-v1`, `halt-none-v1`
  silent; `push.rs` `halt_channel`), send `haltSounds: true` and the phone's picks in
  `push.register`, and add the picker (with a preview player) to the phone's Settings. The server
  already routes a phone that says `haltSounds: true` to its sound's channel and names
  `halt_<key>.wav` for iOS; every other phone keeps the default sound.

## The rule

**Everything chart-related lives in the chart packages; an app passes data in and wires callbacks
out** (owner rule, 2026-10-17). A screen never draws candles, computes an indicator, hit-tests a
drawing or formats a price itself. If a new screen needs chart behaviour that is not there, add it to
the packages (both renderers) rather than to the screen.

## Layers

```
            @stockwatcher/shared          protocol types, WS client, settings sync engine, ticker search
                     │
            @stockwatcher/chart-core      framework-free: ChartStore, scales, layout → display list,
                     │                    gestures, indicators, drawings, levels, trading, settings models
          ┌──────────┴───────────┐
 @stockwatcher/chart-web   @stockwatcher/chart-native     two renderers of the SAME display list
 React + Canvas 2D         React Native + Skia            (+ each platform's controls and sheets)
          │                      │
 @stockwatcher/app-state ────────┤                        one Zustand store per client: synced and local
          │                      │                        app state (app-state.md)
     apps/web (+ desktop)   apps/mobile
```

- **chart-core** turns `(series, view, settings) → display list` (rects, lines, paths, text; no
  drawing API calls) and owns every rule: candle and gap handling, price scales, indicator maths,
  drawing geometry and hit tests, order and position lines, gesture recognition. It is pure TS, no DOM,
  no React, and unit-tested (352 tests).
- **chart-web** and **chart-native** only execute that display list and host the platform's controls.
  The same gesture controller drives mouse/touch on the web and Gesture Handler touches on the phone,
  so a chart behaves the same everywhere.
- **app-state** holds what the app knows (ticker, timeframe, indicators, drawings, sheets, …) and syncs
  the person's choices across devices. The chart packages never import it: an app reads the store and
  passes values to the chart as props.

## What the chart can do (inventory)

| Area | Contents | Where |
|---|---|---|
| Timeframes | `15s`, `30s`, `1m`, `5m` (server-aggregated; the client never re-aggregates) | `chart-core/types.ts` `TIMEFRAMES` |
| Chart types | candles, hollow, bars, Heikin-Ashi (display only: price label, readout and orders use real prices), line, area, baseline | `chartTypes.ts` |
| Price scale | linear (price), linear (%), logarithmic; manual price range by dragging the axis | `scale.ts`, `PriceAxisMenu` |
| Zoom | auto stretch, presets 25 / 50 / 100 % of the trading day, pinch/wheel/drag pan with momentum | `chartSettings.ts`, `gestures.ts` |
| Show | extended hours, volume, grid, extended-hours shading, session breaks, high/low labels; visibility of indicators, drawings, orders | `chartSettings.ts` |
| Indicators | SMA, EMA, session VWAP, Bollinger, RSI (Wilder), MACD; at most 3 overlays + 2 panes (colour-validated slots). Computed by the server when it advertises `indicators.v1` (2026-10-31): the host sends the chart's `indicatorDemand` on `chart.subscribe` / `chart.indicators.set`, `ChartStore` keeps the pushed values (`getIndicators`) and `serverIndicators.ts`' `serverIndicatorResults` joins them onto the shown slots. The client computes none: the TS math (`computeIndicators` and the six formulas) is in `indicatorMath.ts`, outside the package's exports, only for the Rust parity fixtures and tests; a server without the feature gets no lines and a "needs a server update" note. The phone asks for the newest 1500 bars (`limit`, `indicators.limit.v1`) until zoomed out past them | `indicators.ts`, `serverIndicators.ts`, `store.ts` |
| Drawings | trend, ray, horizontal, vertical, rectangle, Long and Short position plans; a measure tool (not stored; also Shift+drag on the web); per-symbol undo/redo; colours | `drawings.ts`, `position.ts`, `toolbox.ts` |
| Levels (layers) | session volume profile, halts, LULD bands, premarket high/low, day range, open, opening range, previous close, supply/demand zones, fills. The key levels, opening ranges and zones are computed by the server when it advertises `levels.v1` / `zones.v1` (2026-10-31): the host asks with `levels` / `zones` (the zones layer on, in `indicatorDemand`), `ChartStore` keeps the pushed `chart.levels` / `chart.zones` and the chart draws them (`serverKeyLevels`); `keyLevels`, `openingRanges` and `autoZones` are only the fallback for an older server and the fixture feed | `levels.ts`, `autoZones.ts`, `store.ts` |
| Paper trading | order and position lines (drag to modify), order ticket, right-click trade menu (web) / quick trade sheet (phone), bracket orders from a position plan | `trading.ts`, `chart-trading-sim.md` |
| Inspection | candle readout (time, OHLC, change, volume, indicator values), "Live" badge on the forming candle | `inspect.ts`, `CandleInspector` |
| Settings UI | Settings dialog (web) / sheet (phone) with Chart and Drawing tabs, single column with drill-down on phones | `settingsDialog.ts`, `SettingsDialog`, `SettingsSheet` |
| Icons | one duotone icon set (primary + 40 % secondary; accent duotone when active) | `icons.ts`, `Icon` in each renderer |

## Embedding a chart

### Web (`@stockwatcher/chart-web`)

```tsx
import { CandleChart } from "@stockwatcher/chart-web";

<CandleChart
  // data
  series={series}            // ChartSeriesLike: pass a NEW object whenever it changes (useChartFeed does)
  timeframe={timeframe}
  symbol={symbol}
  now={clock?.now ?? null}
  // controlled view and preferences: value + onChange pairs (omit the pair to let the chart keep it)
  settings={chartSettings} onSettingsChange={setChartSettings} settingsFromRemote={fromRemote}
  indicators={indicators}   onIndicatorsChange={setIndicators}
  drawings={drawings}       onDrawingsChange={setDrawings}
  layers={layers}           onLayersChange={setLayers}
  inspectT={inspectT}       onInspectChange={setInspectT}
  tradeQty={tradeQty}       onTradeQtyChange={setTradeQty}
  drawingSettings={drawingSettings}
  onTimeframeChange={setTimeframe}   // shows the timeframe picker in the toolbar
  trading={trading}                  // ChartTrading, or null/absent = no trading
  // chrome
  toolbar inspector toolbarInChart heading={<Ticker />}
  persistKey="sw-chart"              // where the chart keeps what you did not control (layout)
  options={{ fontSize: 9 }} barSpacing={10} label="AAL 1 minute candles"
/>
```

- **Controlled or not, per pair.** Every preference is a `value` + `onXChange` pair. Pass both and the
  host owns it (synced through the app store in our apps); omit the callback and the chart keeps it
  itself, persisted under `persistKey`. Screen layout (toolbox position, pane heights, tools shown)
  is left to the chart on the web.
- **Composable pieces** for screens around the chart: `ReplayControls`, `SessionTimeline`, `Tape`,
  `VolumeProfile`, `TimeframeSelector`, `TimeframeWheel`, `CandleInspector`, `ChartToolbar`, `OrderTicket`,
  `PositionPlanCard`, `TradeMenu`, `LayersMenu`, `TradeHistory`, `SettingsDialog` with
  `chartSettingsSection` / `drawingSettingsSection`, `Icon`. UI primitives (shadcn on Base UI) are at
  `@stockwatcher/chart-web/ui`.
- **Host requirements:** Tailwind v4 with the package in `@source`; the shadcn token names mapped onto
  the 60/30/10 palette (`bg-primary`, `text-muted-foreground`, `text-text-faint`, `bg-bg-row`,
  `bg-bg-raised`, `text-warning`, `text-critical`, …); optional `--chart-*` CSS variables theme the
  canvas (`theme.ts`). See `apps/web/src/index.css`.

### Phone (`@stockwatcher/chart-native`)

The same `CandleChart` API, plus the props the web keeps internally: `toolboxPlacement` /
`onToolboxPlacementChange`, `toolsVisible` / `onToolsVisibleChange`, `paneHeights` /
`onPaneHeightsChange` (the phone's app store keeps these per device), `profileBins` / `profileBin` /
`onProfileBinChange` (the profile lives inside the chart on a phone), and `haptics`. Pieces:
`TimeframeWheel`, `SessionTimeline`, `TapeSheet`, `ReplayDaySheet`, `SettingsSheet`,
`PositionPlanPanel`, `QuickTradeSheet`, `Segmented`, `ToolButton`, `Icon`, `UI` (colours).

Requirements: `@shopify/react-native-skia`, `react-native-gesture-handler`, `react-native-reanimated`,
`react-native-svg`, `expo-haptics`; a **development build** (not Expo Go). Native components may use
only RN core plus those.

### Data: feed → store → props

```
server /ws ──► ChartFeed.subscribe({symbol, timeframe, profileBin}, emit)   (apps/*/src/feed)
                    │ chart.snapshot / chart.update / chart.remove / chart.gap / chart.halt / chart.luld
                    ▼                 / tape.trade / replay.clock
               useChartFeed(feed, symbol, timeframe, bin)  →  chart-core ChartStore
                    │  (useSyncExternalStore; a new Series object per change)
                    ▼
               { series, tape, clock }  →  <CandleChart series={series} … />
```

- `ChartFeed` (`apps/web/src/feed/types.ts`) is the server feed (tests use fakes). There is no
  in-browser fixture since 2026-10-31: an unreachable server shows "Server unreachable" and is
  retried every 5 s (`ConnectionGate`), and development runs the server's replay over its fixture
  day (`SIM_AUTOFIXTURE=1 bun dev`). The same feed also carries the paper
  broker (`feed.paper`, wrapped by `usePaperTrading`) and the quote board (`feed.quotes`, all
  price-band candidates' last price, change reference and volume, at most once a second; see
  `simulator-and-chart.md` "Quote board").
- The candle list, gaps, profile, halts and LULD per symbol and timeframe live in chart-core's
  `ChartStore`; the clock is the server's (`replay.clock`).
- App state (which symbol, timeframe, indicators…) comes from the app store, not from here
  (`app-state.md`).

## Before building the live dashboard: constraints to plan around

1. **Up to 4 charts per connection (solved 2026-10-18).** `chart.subscribe` takes a slot `id`; the
   server streams up to 4 slots on one socket and sends each frame once per distinct chart
   (`crates/server/src/ws.rs` header). Clients route frames back to their slots with chart-core's
   `createChartRouter` (by symbol, timeframe and bin), and `useChartFeed(feed, symbol, tf, bin, slot)`
   takes the slot. The dashboard uses `d0`…`d3`; the chart page keeps the default slot. On the phone
   each tab has its own socket (`useConnection`), because the chart tab's slot plus the dashboard's 4
   would exceed the limit while both tabs stay mounted.
2. **Live and replay run side by side (solved 2026-10-18).** The owner's direction: the **main page
   consumes only live data**; **replay is a separate practice feature** (like TradingView's Bar Replay
   or Webull paper trading). The server starts two markets (`Markets` in `crates/server/src/market.rs`):
   live whenever the Alpaca keys are set, replay on the archive. Each socket picks one for its life in
   `hello.source` (`live` | `replay`) and each market REST route takes `?source=`; clients that name
   neither get `MARKET_DATA_MODE`, so apps built before the split are unchanged. The web
   `ConnectionGate` and the phone's `useConnection` take the source: the dashboard asks for `live`, the
   chart page for `replay`. Each market has its own paper account. The replay clock (day, speed,
   play/pause) is still one shared practice session, not one per person. An empty live list is real
   (closed, or before the first trade): the live page shows "No live trades yet", never the fixture.
3. **Live mode records into the archive (solved 2026-10-18): one store, read two ways.** The live
   market writes what it receives into the archive itself, `ARCHIVE_ROOT/<trading date>/live/`, a
   **provisional** day the nightly SIP import replaces with the complete, corrected one (the import
   wins; live never writes into an imported day). The dashboard is that history plus the live tail:
   after a restart (every push to `main` redeploys) the market is `running` at once with the quote
   board restored from the recording's saved rows, each symbol's day is loaded into the engine when
   a chart asks for it (an empty chart first, then the real one) and every other symbol in the
   background, and the paper account never sees history. When nothing trades (a weekend, before
   the overnight session) it shows the **last session** the archive has, labelled with its date
   ("Last session · Fri Oct 16", from `session` in `/api/symbols`), and rolls into the new session
   with its first trades. Replay offers a recorded day, marked provisional, once its session is
   over. Still lost: the **restart gap** (the old process's last flushed second to the new
   stream's first event), not backfilled until the import. Design and numbers:
   `simulator-and-chart.md`, "Live recording".
4. **Known client bug:** the replay bar's clock label stops updating while replay plays, although the
   server sends `replay.clock` about 4 times a second (seen on `main`, 2026-10-18; not yet fixed).
5. **Dashboard state goes in the app store** (done for the panel: the synced `dashboard` namespace). A watchlist, the dashboard layout or a selected symbol
   that should follow the person across devices is a new synced namespace in `@stockwatcher/app-state`
   (`app-state.md` "New synced value"); per-screen layout is device state.
6. **New reusable UI goes into wavykit too** (workspace rule): a watchlist row, a mini chart card or a
   quote tile that a second screen could use is a component, not a one-off layout.
