//! `/ws`: JSON text frames. Mirrors packages/shared/src/protocol.ts.
//!
//! The client must send `hello` (with the token when auth is on) within
//! HELLO_TIMEOUT; `hello.source` (`live` | `replay`, absent = `MARKET_DATA_MODE`) picks which
//! of the two markets the connection reads for its whole life (crate::market::Markets); the server answers `welcome`, then pings every HEARTBEAT
//! and drops a client that has been silent for 3 heartbeats.
//!
//! Chart protocol (after `welcome`; crates/chart-engine wire types, mirrored by
//! packages/chart-core/src/types.ts):
//! - client `chart.subscribe {symbol, timeframe, profileBin, id?}` -> server
//!   `chart.snapshot`, the recent `tape.trade` rows, then increments
//!   (`chart.update` / `chart.remove` / `chart.gap`) for that symbol and
//!   timeframe, and `tape.trade`, `chart.halt` and `chart.luld` for the symbol
//!   whatever its timeframe (docs/architecture/halts-luld-fills.md).
//! - **Up to MAX_CHARTS charts per connection** (2026-10-18, the dashboard's multi-chart
//!   panel). `id` names a chart slot (≤ MAX_CHART_ID chars; absent = `""`, the single slot every
//!   client used before): a subscribe with an id already held replaces that slot's chart (e.g.
//!   another timeframe) and starts with a fresh snapshot of the same engine state; a new id when
//!   MAX_CHARTS are held is refused with `error too_many_charts`. Client
//!   `chart.unsubscribe {id?}` frees a slot. One socket for every chart, rather than one per
//!   chart, so the market stream is filtered once and paper, settings and clock frames are not
//!   repeated per chart.
//! - **Each frame goes out once per connection**, whatever the number of slots: a candle frame
//!   once per distinct (symbol, timeframe, bin) held, a tape, halt or LULD row once per symbol.
//!   Two slots showing the same chart share one stream; the client routes frames by the symbol,
//!   timeframe and `profileBin` they already carry, not by slot.
//! - A candle frame whose `profile` rows are non-empty also carries the whole profile's `total`,
//!   `poc` and `valueArea` at that frame's bin (chart_engine::wire::ProfileDerivedWire; additive,
//!   older clients ignore it): crate::profile_frames.
//! - Chart errors (`unknown_symbol`, `outside_price_band`) carry the `symbol` and, for a named
//!   slot, its `id`, so a client with several charts knows which one was refused.
//! - server `replay.clock {clock}` to every client, on change and ~4 Hz while
//!   playing; in live mode once a second (the live provider's tick). `clock.nowNs` (exchange
//!   time, ns) rides beside `clock.now` (ms).
//!
//! Exchange-ns times (`times.ns.v1` in `welcome.features`; crate::time_marks, chart_engine::wire):
//! every ms time on the wire has its ns twin beside it (`tNs`, `startNs`, `endNs`), additive and
//! always sent; clients prefer it, older ones read the ms field as before.
//!
//! Server clock (`clock.v1` in `welcome.features`; crate::time_marks): Rust is the only clock.
//! - `rocket.*` rows carry `fresh` / `fresh_until_ns`, and a burst row `burst_live` /
//!   `burst_live_until_ns` and `flame {lit, until_ns}`, decided at the market clock; a flip sends
//!   a `rocket.update` on the next tick. Clients show them as sent and measure ages against the
//!   last `replay.clock`, without extrapolating it.
//! - server `runners.event {kind: new_flag | breakout, id, symbol, at_ns, path, price,
//!   from_flag_pct?, reason?}` to `rocket.subscribe`d connections, live market only, at most 3 a
//!   tick, judged fresh on the market clock: the web dashboard's Runners toasts. Older clients
//!   ignore the unknown type and keep diffing panels.
//! - `alerts.state` entries carry `snoozed`, with expiry and the cooling gap decided on the
//!   evaluator's 1 s tick; `paper.state.notice.fresh` (Alpaca paper) is judged when rendered.
//! - client `replay.control {action, value}` (play, pause, step, stepInterval,
//!   seek, speed, day): honoured in replay mode, `error not_replay` in live mode.
//!   Success has no direct reply; the resulting `replay.clock` is the answer.
//! - `day` (value `"YYYY-MM-DD"`, one of `GET /api/replay/state`'s `days`)
//!   switches the replayed day: a rebuild exactly like a seek (paper account
//!   reset, tape and quote board rebuilt), on the new day at the configured
//!   `REPLAY_START` time of day, playing or paused as before. A day the
//!   archive does not offer gets `error unknown_day`, one whose files cannot
//!   be opened `error day_unavailable`; either way nothing changes.
//! - A client that falls more than the broadcast capacity behind, and every
//!   client after a seek or a day switch, gets a fresh `chart.snapshot`
//!   (resync). After a day switch the subscribed symbol may not exist on the
//!   new day: the resync then answers `error unknown_symbol` (or
//!   `outside_price_band`), and the client picks a symbol from `/api/symbols`,
//!   which already lists the new day's candidates.
//!
//! Chart indicators computed on the server (`indicators.v1`, crates/server/src/indicators.rs,
//! the math in crates/chart-engine/src/indicators.rs):
//! - `welcome.features` lists `"indicators.v1"`. A client sends indicators only when it sees it,
//!   so a new client on an old server keeps its local math.
//! - `chart.subscribe` takes two optional fields: `indicators` (the slot's effective list, the
//!   `IndicatorSpec` shape of packages/chart-core: `{id, kind, period?, mult?, fast?, slow?,
//!   signal?}`, parsed like the client's `parseIndicators`, overlays past 3 and panes past 2
//!   dropped in order) and `rth` (true = extended hours off: regular session only). Without
//!   `indicators` (every phone build installed before `indicators.v1`) nothing below is sent.
//!   With it, `chart.indicators` follows the `chart.snapshot` (same seq) and `chart.indicator`
//!   increments follow the candle frames. A list that is not a list gets `error bad_request`
//!   with the slot's `symbol` and `id`.
//! - client `chart.indicators.set {id?, indicators, rth?}` changes a held slot's studies without
//!   a new candle snapshot (a study toggled, extended hours flipped, indicators hidden = `[]`);
//!   the answer is a `chart.indicators`. An `id` no slot holds gets `error bad_request`.
//! - `limit` (a bar count, `indicators.limit.v1` in `welcome.features`) on `chart.subscribe` and
//!   `chart.indicators.set`: the slot's `chart.indicators` carries only the newest `limit` bars
//!   (at least crate::indicators::MIN_LIMIT), with `truncated: true` when bars were left out.
//!   The values are the same as without it (the series run over every retained bar); the phone
//!   asks for fewer on cellular until it is zoomed out past them. Absent = every bar. A socket's
//!   slots on one chart key share one frame, so the largest limit wins (none if any has none),
//!   and the snapshot a deep revision broadcasts (every bar) reaches a limited socket as its own
//!   limited snapshot instead.
//! - Frames carry canonical spec keys (`ema:9`, `bb:20:2`, `macd:12:26:9`), never client ids;
//!   the client maps its specs to keys (chart-core `specKey`). Shapes and the apply rules are in
//!   the header of crate::indicators. A lag, a seek or a day switch resends `chart.indicators`
//!   with the snapshot, as for the candles.
//!
//! Session schedule (`sessions.v1`; chart_engine::wire::ChartSnapshot::sessions): every
//! `chart.snapshot` carries `sessions`, the NYSE calendar's sessions for each trading date its
//! candles span plus the next one (early closes: regular to 13:00, after-hours to 17:00).
//! Clients filter regular hours and shade by it; an older server's snapshot lacks it and the
//! client keeps the fixed 09:30-16:00 schedule.
//!
//! Key levels, opening ranges and supply/demand zones computed on the server (`levels.v1`,
//! `zones.v1`; crate::chart_context, the levels math in crates/chart-engine/src/levels.rs):
//! - `chart.subscribe` takes `levels: true` (sent only when the welcome lists `levels.v1`) and
//!   `zones: true` (only with `zones.v1`, and only while the chart shows the zones layer; the
//!   zones are over the regular session with `rth`). Without them (every client from before) no
//!   frame below is sent and the client keeps computing its own.
//! - `chart.levels` follows the slot's `chart.snapshot`, and `chart.zones` when asked; both are
//!   whole states (shapes in crate::chart_context), broadcast again whenever their content
//!   changes and resent on every resync. `chart.indicators.set` takes `zones?` too: turning the
//!   layer on (or flipping `rth` with it on) answers with a `chart.zones`; absent = unchanged.
//!
//! Price-band candidates (`symbols.v1` in `welcome.features`; crate::symbols, after `welcome`):
//! - client `symbols.subscribe` -> server `symbols.snapshot {rev, symbols, mode, status, source,
//!   day, fixture, priceBand, screened, session}` (the fields of `GET /api/symbols`), then
//!   `symbols.update {rev, added, removed, ...the same meta}` when the set or the meta changed,
//!   checked once per wall second. An update applies to the list at `rev - 1`; a gap means a
//!   missed frame: subscribe again. `symbols.unsubscribe` stops it; a seek or a lag resends the
//!   snapshot. Clients that never subscribe (older builds) keep polling `/api/symbols`.
//!
//! Notification log (`notices.v1` in `welcome.features`; crate::notices, after `welcome`):
//! - client `notices.subscribe {day?}` ("YYYY-MM-DD"; absent = today in ET) -> server
//!   `notices.snapshot {day, days, items, truncated}` (the body of `GET /api/notices`), then for
//!   that day `notices.append {day, items: [item]}` per ledger line: the notice's whole item so
//!   far, upserted by `event_id`. Without `day` it follows today into the next date (a fresh
//!   snapshot). A past day gets the snapshot only. A malformed day gets `error bad_request`.
//!   `notices.unsubscribe` stops it; a lag resends the snapshot. Older clients keep polling.
//!   With `notices.rows.v1` also listed, every item carries `status`/`reason`/`reason_text` and
//!   every body a `summary` (crate::notices::FEATURE_ROWS); additive.
//!
//! Insights ticker page (`insights.ticker.v1` in `welcome.features`; crate::insights):
//! - client `insights.ticker.subscribe {symbol, day?}` (day absent = today's trading date) ->
//!   server `insights.ticker.snapshot {symbol, day, …}` (the body of `GET /api/insights/ticker`);
//!   for today then `insights.ticker.update {…}` (the whole package, replacing) whenever it
//!   changes. A new subscribe replaces the old one, and a past day's snapshot still being built
//!   for the replaced one is dropped, never sent late (clients may still match `symbol`/`day`).
//!   `insights.ticker.unsubscribe` stops it. A malformed request gets `error bad_request`.
//!
//! Simulated trading (docs/architecture/chart-trading-sim.md, after `welcome`):
//! - server `paper.state` once right after `welcome`, then to every connection
//!   on any change, with a `paper.fill` per fill. A lagged client gets a fresh
//!   `paper.state` too, since it may have missed one.
//! - client `order.place`, `order.modify`, `order.cancel`, `paper.reset`.
//!   Success has no direct reply; the broadcast `paper.state` is the answer.
//! - Live market with the `tradingAccount` setting on `alpaca_paper` (crate::trading,
//!   docs/architecture/alpaca-paper.md): the same commands go to the owner's Alpaca PAPER
//!   account, and `paper.state` / `paper.fill` carry `simulated: false`, `broker:
//!   "alpaca_paper"`. A refused modify/cancel gets `error` with Alpaca's text (`not_open`,
//!   `broker_rejected`, `rate_limited`, `not_configured`); `paper.reset` gets `error
//!   not_supported`. The replay market is always the simulator (`broker: "simulator"`).
//!   A malformed or unknown `order.*`/`paper.*` command gets `error bad_order`;
//!   a refused modify/cancel gets `error` with the broker's code (`not_open`, ...).
//!
//! Quote board (the ticker search's live rows; crates/server/src/quotes.rs, after `welcome`):
//! - client `quotes.subscribe` (no payload) -> server `quotes.snapshot {t, quotes: [row]}` at
//!   once with every price-band candidate that has traded, then `quotes.update {t, quotes: [row],
//!   gone?: [symbol]}` with only the rows that changed, at most once per second of wall time
//!   (crate::market::QUOTES_INTERVAL; nothing is sent when nothing changed). `gone` lists symbols
//!   that left the price band. `t` is the replay/live clock (epoch ms) at the take.
//! - Row: `{s, p, pc?, pcb?, o, v, n, pre?, name?, x?}`: symbol, last price, previous close (the
//!   change-% reference whenever present; from the archive's daily-closes.json, put on today's
//!   share basis when the corporate-actions ledger knows a split since: `pcb: "split"`; withheld
//!   when the day's first print is a typical split factor away from it and no split is known:
//!   `pcb: "guard"`, and the client must then measure against `o` even if it holds an older
//!   `pc`; crate::quotes::PcBasis), the fallback
//!   reference (the regular open, or the day's first price while `pre` is true), day volume,
//!   prints counted; `name`/`x` (exchange) only in snapshots and for rows entering the band, and
//!   only when the archive's assets.json has them. Prices are JSON numbers in dollars. Change % is
//!   `(p - pc) / pc`, else `(p - o) / o`, computed by the client.
//! - client `quotes.unsubscribe` stops the updates. A subscribe while subscribed re-sends the
//!   snapshot. Subscribing again after a reconnect is the client's job, as for charts.
//! - After a seek (rebuild) and for a lagged connection, a subscribed client gets a fresh
//!   `quotes.snapshot` (it replaces the client's board); no updates go out during a rebuild.
//!
//! Ticker search ranked on the server (`quotes.search.v1` in `welcome.features`;
//! crate::quote_search, whose header has the shapes):
//! - client `quotes.search {id?, filters?, sort?, limit?, pins?}` -> server
//!   `quotes.search.snapshot {id, t, total, rank, rows}`, then `quotes.search.update {id, t, rows,
//!   total?, rank?, gone?}` at most once per second of wall time when the result changed. Rows
//!   carry the server's `chg` (change %). A new `quotes.search` with a held id replaces it;
//!   `quotes.search.stop {id?}` ends it; at most crate::quote_search::MAX_SEARCHES per connection
//!   (`error too_many_searches`), an unknown chip or sort is `error bad_request` with the `id`.
//!   A seek or a lag resends each search's snapshot.
//! - client `quotes.directory` -> server `quotes.directory {rows: [[symbol, name?, exchange?]]}`:
//!   the candidates' names, for the client's fuzzy matching (which stays on the client).
//! - A client that uses these no longer sends `quotes.subscribe`; older clients still do.
//!
//! Halt watcher (the dashboard's build-up board; crates/server/src/pressure.rs, after `welcome`):
//! - client `pressure.subscribe` (no payload) -> server `pressure.snapshot {t, board: [card]}` at
//!   once with the last ranked board (empty for up to a second after start or a rebuild), then
//!   `pressure.update {t, board: [card]}` with the WHOLE board (at most 6 cards, usually fewer,
//!   often none, in display order) whenever it changed, at most once per second of wall time.
//!   Either frame replaces the client's board. `t` is the market clock (epoch ms) at the ranking.
//!   A card exists only while a symbol's flow pressure is building up toward its LULD band.
//! - Card: `{s, p, pc?, o, risk, side, rise, riseSecs, band, toBand, pressure, buyShare, pace,
//!   halt?}`: symbol; last price, previous close and fallback reference as in a quote row (change
//!   % by the same rule); halt risk 0–100 (the gauge); `bull` | `bear`, the side the build-up
//!   pushes (toward the upper or the lower band); the rise of that side's pressure over the last
//!   `riseSecs` seconds; the modelled LULD band on that side and the distance to it (% of the
//!   price); the flow pressure 0–100 with its buy share (0–1) and pace (× the symbol's average
//!   volume per minute this session); `halt {start, reason}` while halted (everything frozen at
//!   the halt).
//! - client `pressure.unsubscribe` stops the updates. Resubscribing after a reconnect is the
//!   client's job; a seek or a lag sends a fresh snapshot, as for quotes.
//!
//! Ross Panel and stock context (`STOCK_CONTEXT=1`; crates/server/src/context.rs,
//! docs/stock-context/*.md, after `welcome`):
//! - client `ross.subscribe` -> server `ross.snapshot {enabled, panelPolicy, pillarPolicy, panel}`
//!   at once, then `ross.update` (same shape, the WHOLE panel) when the panel changed, at most once
//!   per second of wall time. `enabled: false` (feature off) carries `panel: null`: "not enabled",
//!   never "nothing qualifies". `panel` is null until the first barrier with members, then
//!   `{revision, runId, source, barEnd, cutoff, rows: [row]}` (snake_case fields, see
//!   `stock_context::PanelSnapshot`): rows ranked by momentum first, unranked rows after.
//! - Each row may carry its day line (crate::day_line): `day_line_start`, `day_line_step_s`
//!   (30), `day_line_from`, `day_line` (micros from that index) and `prev_close`. A snapshot sends
//!   whole lines; an update sends only what changed since the previous update, which the client
//!   merges. A slice it cannot place: drop the line and send `ross.subscribe` again (a fresh
//!   snapshot answers every subscribe).
//! - client `ross.unsubscribe` stops the updates; a seek or a lag sends a fresh snapshot.
//! - client `context.get {symbol}` -> server `context.decision {symbol, enabled, decision}`: the
//!   symbol's latest stock-context decision (five pillars with values, rules, reasons, evidence),
//!   or `null` before its first trade.
//!
//! Favorites (crates/server/src/favorites.rs, after `welcome`; the list is the synced settings
//! namespace `favorites`):
//! - client `favorites.subscribe` -> server `favorites.snapshot {t, symbols, rows}` at once, then
//!   `favorites.update` (same shape; day lines as slices, as in `ross.update`) when a row or the
//!   list changed, at most once per second. The row fields are in that module's docs.
//! - client `favorites.unsubscribe` stops the updates; a seek or a lag sends a fresh snapshot.
//!
//! Movers: Top Gainers and Hot stocks Bull / Bear (crates/server/src/movers.rs, after `welcome`;
//! ranked once a tick on the server for every client, so a dashboard no longer holds the whole
//! quote board):
//! - client `movers.subscribe` -> server `movers.snapshot {t, gainers, bull, bear, rows}` at once
//!   (the lists as symbol arrays in rank order, at most 25 each; one row per symbol in any list,
//!   with its whole day line), then `movers.update {t, rows, gainers?, bull?, bear?, gone?}` when
//!   anything changed, at most once per second of wall time: only the lists that changed (whole),
//!   only the rows that changed, entered or gained day-line points (lines as slices, as in
//!   `ross.update`), and `gone` for symbols in no list any more. The row fields are in that
//!   module's docs.
//! - client `movers.unsubscribe` stops the updates; a seek or a lag sends a fresh snapshot.
//!
//! Rocket watch (EXPERIMENTAL, `ROCKET_WATCH`, on by default; crates/server/src/rocket/, after
//! `welcome`):
//! - client `rocket.subscribe` -> server `rocket.snapshot {t, enabled, strategy, mode, budget,
//!   threshold, today, days, rows}` at once, then `rocket.update` (same shape, the WHOLE panel
//!   except the day lines, which are slices as in `ross.update`) when it changed, at most once per
//!   second of wall time. `enabled: false` (feature off) has no
//!   strategy and no rows: "not enabled", never "nothing flagged".
//! - `strategy {id, version, placeholder}`; `mode` `budget` | `fixed`; `budget` the alerts-per-day
//!   target; `threshold` the score an alert needs right now (null before one exists);
//!   `today {date, alerts, hits, hits30, known}` (hits = a rocket started 60–600 s after the
//!   alert, the bake-off's HIT; `known` = alerts whose outcome is known); `days [{date, alerts,
//!   hits10, hits30}]` the last 20 trading days before today, oldest first.
//! - Row (newest first, at most 50): `{id, s, at, p, last, pc, score, reasons, outcome}`: symbol,
//!   alert time (epoch ms), price at the alert, last price now and previous close (day gain
//!   `(last - pc) / pc`, null when unknown), the strategy's score, 1–3 human-readable reasons,
//!   `outcome {state, gain, drawdown, partial, gain_2h, drawdown_2h, gain_session,
//!   rocket_later}` with state `pending` | `rocketed` | `late` | `moved` | `faded`, the max gain /
//!   drawdown (%) over the 30 minutes after the alert, over 2 hours, the max gain to 20:00 ET, and
//!   whether a +30 % rocket mark came any time later (crate::rocket::track); `today.ran_later`
//!   counts rows that did not rocket strictly but gained ≥ 15 % within 2 hours.
//! - Momentum (additive, crate::rocket::track): `state` `watching` | `building` | `breaking_out`
//!   | `running` | `pullback` | `fading` | `halted`, `state_since` (epoch ms), `provisional`
//!   (a breakout seen in the forming bar, or a halt not yet at a barrier), `from_flag_pct`,
//!   `from_high_pct` (≤ 0, against the high since the flag), `move_1m_pct`, `pullback_depth_pct`
//!   (in `pullback`), `levels {state, trigger, invalidation, meter_milli, distance_to_trigger_bp,
//!   to_invalidation_bp, near_trigger, episode_id}` (the micro-pullback MP5 cue, prices in
//!   micros; null without `STOCK_CONTEXT` or before a level exists), and the day line fields
//!   (`day_line_start`, `day_line_step_s`, `day_line_from`, `day_line`, `prev_close`) of
//!   crate::day_line.
//! - client `rocket.unsubscribe` stops the updates; a seek or a lag sends a fresh snapshot.
//!
//! Synced settings (crates/server/src/settings.rs, docs/architecture/settings-sync.md, after
//! `welcome`): the user's chart preferences and drawings, shared by every client.
//! - server `settings.snapshot {docs: [{ns, rev, updatedAt, value}]}` once right after `welcome`
//!   (after `paper.state` and the clock), with EVERY namespace (drawings are small, so all symbols'
//!   drawings come at once rather than per chart subscription). It is the whole truth: a client
//!   replaces its copy of each namespace listed. Sent again to a lagged connection; a reconnect
//!   gets one like any new connection.
//! - client `settings.set {ns, value, opId, ifAbsent?}` replaces a namespace's value; with
//!   `ifAbsent: true` only if it has never been written (a client's one-time migration of its
//!   local settings: the first client to connect seeds the shared value).
//! - client `settings.patch {ns, set?: {key: value}, remove?: [key], opId}` sets/removes entries
//!   of an object-valued namespace (a symbol's drawings by id, `drawings:<SYMBOL>`), so edits of
//!   different drawings from two devices never overwrite each other.
//! - server `settings.changed {ns, rev, updatedAt, value, opId}` to EVERY connection, the writer
//!   included, after each applied write (whole value, also after a patch). Last writer wins by
//!   arrival; `rev` climbs by one per namespace; the writer recognises its `opId`. An `ifAbsent`
//!   set that found a value answers only the writer, with the current document and its opId
//!   (rev unchanged).
//! - server `settings.rejected {ns, opId, code, message}` to the writer alone: `bad_request`,
//!   `too_large`, `store_full`, `not_a_map` (drop the edit) or `settings_unavailable` (the disk
//!   write failed: keep it and resend after the next reconnect). Nothing changed.
//! - Values are opaque JSON: the clients own the schemas and validate what they read.
//! - `broker.status` is server-written too (crate::trading::STATUS_NS): the Alpaca paper route's
//!   health, refused with `bad_request` like `alerts.state`.
//! - `alerts.state` is the one namespace only the server writes (crate::alerts::model::STATE_NS):
//!   a client `settings.set`/`settings.patch` on it is refused with `bad_request`.
//!
//! Price alerts (crate::alerts, after `welcome`, on every connection whatever its market):
//! - server `alerts.history {events, phoneToday, phoneBudget, devices, dropped}` once right after
//!   `settings.snapshot` (Activity: the newest 500 fires and hold-backs, oldest first), and again
//!   to a connection that lagged on the alert frames.
//! - server `alert.fired {event, phoneToday}` and `alert.held {event}` as they happen, to EVERY
//!   connection; `alert.updated {event}` when a recorded event changes (a phone push that failed
//!   after it was recorded); `alerts.status {phoneToday, phoneBudget, devices, dropped}`.
//! - client `push.register {token, platform?, name?, runnersNew?, runnersBreakout?, runnersBurst?, runnersRunning?, runnersRun?, runnersFast?, runnersShelf?, runnersFollowUp?, rossCrossing?, signalsCrossing?, rossTrigger?, signalsTrigger?, haltWatch?}`: this
//!   phone's Expo push token, kept by the server for phone notifications and never sent to other
//!   clients, with its Runners push opt-ins (crate::alerts::runners; an absent one keeps what the
//!   phone had; for a new phone breakouts off, the rest on); a malformed token gets
//!   `error bad_request`. `push.unregister {token}` removes it. No direct reply on success (the
//!   `alerts.status` frame's `devices` count moves).
//! - The rules themselves are the synced `alerts` namespace (settings.* above).
//! - client `runners.notify {enabled?, new?, breakout?, burst?, running?, run?, fast?, shelf?,
//!   followUp?, rossCrossing?, signalsCrossing?, rossTrigger?, signalsTrigger?, halt?}`: this
//!   desktop or web connection shows Runners, Ross, Signals and Halt watcher system notifications
//!   for those kinds (absent = the phone's default: breakouts, "At trigger" and the Halt watcher
//!   off, the rest on; `enabled: false` stops). Per connection, for
//!   as long as the socket lives; resend after a reconnect. No reply.
//! - server `runners.notice {id, alertId, symbol, kind, title, body, data, at}` to EVERY
//!   connection when the desk limits let a Runners notice through
//!   (crate::alerts::runners::desk_frame): the phone push's own text. A client shows it only if
//!   its switch for `kind` is on, once per `id`. Older clients ignore the unknown type.
//!   `runners.notice.v1` in `welcome.features` (crate::alerts::runners::NOTICE_FEATURE) says so:
//!   a phone without a push token then opts in and rings from these frames rather than diffing
//!   Runners panels itself.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::extract::State;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade, close_code};
use axum::response::Response;
use serde::{Deserialize, Serialize};
use tokio::time::{Instant, interval_at, timeout};

use crate::chart_context::{ContextKey, LevelsKey};
use crate::indicators::{ChartKey, Studies};
use crate::market::{
    ContextLease, ControlError, DetectionGuard, FavoritesGuard, IndicatorLease, Market,
    MoversGuard, OutKind, PressureGuard, QuotesGuard, RocketGuard, RossGuard, SnapshotError,
    SymbolsGuard,
};
use crate::settings::{SettingsError, SettingsOp, rejected_frame as settings_rejected};
use crate::{AppState, COMMIT, Settings, VERSION, api, auth};
use chart_engine::Timeframe;
use market_events::Price;
use paper_broker::wire::{CancelCommand, ModifyCommand, PlaceCommand};
use tokio::sync::broadcast::error::RecvError;

pub const HEARTBEAT: Duration = Duration::from_secs(15);
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_MESSAGE_BYTES: usize = 64 * 1024;

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum ClientMessage {
    Hello {
        token: Option<String>,
        client: Option<String>,
        /// `live` or `replay`: which market this connection reads (crate::market::Markets).
        /// Absent = the server's default (`MARKET_DATA_MODE`).
        #[serde(default)]
        source: Option<crate::market::Source>,
    },
    Ping {
        ts: u64,
    },
    Pong {
        #[allow(dead_code)]
        ts: u64,
    },
    #[serde(rename = "chart.subscribe")]
    ChartSubscribe {
        symbol: String,
        timeframe: Timeframe,
        #[serde(rename = "profileBin", default)]
        profile_bin: Option<f64>,
        /// The chart slot; absent = the single default slot.
        #[serde(default)]
        id: Option<String>,
        /// The slot's indicator list (`indicators.v1`); absent = the client computes its own.
        /// Raw JSON: parsed as leniently as the client's own `parseIndicators`.
        #[serde(default)]
        indicators: Option<serde_json::Value>,
        /// Extended hours off: the indicators (and the zones) run over the regular session only.
        #[serde(default)]
        rth: Option<bool>,
        /// `chart.levels` for this slot (`levels.v1`); absent = the client computes its own.
        #[serde(default)]
        levels: Option<bool>,
        /// `chart.zones` for this slot (`zones.v1`); absent = none (the layer is off, or the
        /// client computes its own).
        #[serde(default)]
        zones: Option<bool>,
        /// The most bars the slot's `chart.indicators` may carry (`indicators.limit.v1`);
        /// absent = every bar.
        #[serde(default)]
        limit: Option<u32>,
    },
    #[serde(rename = "chart.indicators.set")]
    ChartIndicatorsSet {
        #[serde(default)]
        id: Option<String>,
        indicators: serde_json::Value,
        #[serde(default)]
        rth: Option<bool>,
        /// The zones layer turned on or off (`zones.v1`); absent = as it was.
        #[serde(default)]
        zones: Option<bool>,
        /// As on `chart.subscribe`; absent = every bar (not "as it was": the set is the slot's
        /// whole demand).
        #[serde(default)]
        limit: Option<u32>,
    },
    #[serde(rename = "chart.unsubscribe")]
    ChartUnsubscribe {
        #[serde(default)]
        id: Option<String>,
    },
    #[serde(rename = "replay.control")]
    ReplayControl {
        action: String,
        #[serde(default)]
        value: Option<serde_json::Value>,
    },
    #[serde(rename = "order.place")]
    OrderPlace(PlaceCommand),
    #[serde(rename = "order.modify")]
    OrderModify(ModifyCommand),
    #[serde(rename = "order.cancel")]
    OrderCancel(CancelCommand),
    #[serde(rename = "paper.reset")]
    PaperReset,
    #[serde(rename = "quotes.subscribe")]
    QuotesSubscribe,
    #[serde(rename = "quotes.unsubscribe")]
    QuotesUnsubscribe,
    /// A ranked ticker search (`quotes.search.v1`, crate::quote_search).
    #[serde(rename = "quotes.search")]
    QuotesSearch(crate::quote_search::Request),
    #[serde(rename = "quotes.search.stop")]
    QuotesSearchStop {
        #[serde(default)]
        id: Option<String>,
    },
    /// The candidates' names for the client's fuzzy matching (crate::quote_search).
    #[serde(rename = "quotes.directory")]
    QuotesDirectory,
    #[serde(rename = "pressure.subscribe")]
    PressureSubscribe,
    #[serde(rename = "pressure.unsubscribe")]
    PressureUnsubscribe,
    #[serde(rename = "ross.subscribe")]
    RossSubscribe,
    #[serde(rename = "ross.unsubscribe")]
    RossUnsubscribe,
    #[serde(rename = "favorites.subscribe")]
    FavoritesSubscribe,
    #[serde(rename = "favorites.unsubscribe")]
    FavoritesUnsubscribe,
    #[serde(rename = "movers.subscribe")]
    MoversSubscribe,
    #[serde(rename = "movers.unsubscribe")]
    MoversUnsubscribe,
    #[serde(rename = "symbols.subscribe")]
    SymbolsSubscribe,
    #[serde(rename = "symbols.unsubscribe")]
    SymbolsUnsubscribe,
    /// The Insights log (`notices.v1`, crate::notices): `day` "YYYY-MM-DD", absent = today.
    #[serde(rename = "notices.subscribe")]
    NoticesSubscribe {
        #[serde(default)]
        day: Option<String>,
    },
    #[serde(rename = "notices.unsubscribe")]
    NoticesUnsubscribe,
    /// The Insights ticker page (`insights.ticker.v1`, crate::insights).
    #[serde(rename = "insights.ticker.subscribe")]
    InsightsTickerSubscribe {
        symbol: String,
        #[serde(default)]
        day: Option<String>,
    },
    #[serde(rename = "insights.ticker.unsubscribe")]
    InsightsTickerUnsubscribe,
    #[serde(rename = "rocket.subscribe")]
    RocketSubscribe,
    #[serde(rename = "rocket.unsubscribe")]
    RocketUnsubscribe,
    #[serde(rename = "detection.subscribe")]
    DetectionSubscribe,
    #[serde(rename = "detection.unsubscribe")]
    DetectionUnsubscribe,
    #[serde(rename = "context.get")]
    ContextGet {
        symbol: String,
    },
    #[serde(rename = "settings.set")]
    SettingsSet {
        ns: String,
        value: serde_json::Value,
        #[serde(rename = "opId")]
        op_id: String,
        #[serde(rename = "ifAbsent", default)]
        if_absent: bool,
    },
    #[serde(rename = "settings.patch")]
    SettingsPatch {
        ns: String,
        #[serde(default)]
        set: serde_json::Map<String, serde_json::Value>,
        #[serde(default)]
        remove: Vec<String>,
        #[serde(rename = "opId")]
        op_id: String,
    },
    #[serde(rename = "push.register")]
    PushRegister {
        token: String,
        #[serde(default)]
        platform: String,
        #[serde(default)]
        name: String,
        #[serde(default, rename = "runnersNew")]
        runners_new: Option<bool>,
        #[serde(default, rename = "runnersBreakout")]
        runners_breakout: Option<bool>,
        #[serde(default, rename = "runnersBurst")]
        runners_burst: Option<bool>,
        /// Running-entry pushes (`crate::rocket::running`); on by default for a new phone.
        #[serde(default, rename = "runnersRunning")]
        runners_running: Option<bool>,
        /// "Run +20%" pushes (`crate::rocket::run`); on by default.
        #[serde(default, rename = "runnersRun")]
        runners_run: Option<bool>,
        /// "Fast move" pushes (`crate::rocket::fast`); on by default.
        #[serde(default, rename = "runnersFast")]
        runners_fast: Option<bool>,
        /// `runnersShelf`, `runnersFollowUp`, `rossCrossing`, `signalsCrossing`, `rossTrigger`,
        /// `signalsTrigger`, `haltWatch` (crate::alerts::push::MoreOptIn).
        #[serde(flatten)]
        more: crate::alerts::push::MoreOptIn,
    },
    #[serde(rename = "push.unregister")]
    PushUnregister {
        token: String,
    },
    /// A desktop or web client that shows Runners system notifications, with its switches
    /// (crate::alerts::DeskKinds; absent = the phone's default). `enabled: false` stops it.
    #[serde(rename = "runners.notify")]
    RunnersNotify {
        #[serde(default = "yes")]
        enabled: bool,
        #[serde(default)]
        new: Option<bool>,
        #[serde(default)]
        breakout: Option<bool>,
        #[serde(default)]
        burst: Option<bool>,
        #[serde(default)]
        running: Option<bool>,
        #[serde(default)]
        run: Option<bool>,
        #[serde(default)]
        fast: Option<bool>,
        /// `shelf`, `followUp`, `rossCrossing`, `signalsCrossing`, `rossTrigger`,
        /// `signalsTrigger`, `halt` (crate::alerts::push::MoreOptIn).
        #[serde(flatten)]
        more: crate::alerts::push::MoreOptIn,
    },
}

fn yes() -> bool {
    true
}

/// For a frame in the settings namespace that failed to parse: its `ns` and `opId` if readable,
/// so the rejection names the edit. None for any other frame.
fn settings_frame_ids(text: &str) -> Option<(String, String)> {
    let v = serde_json::from_str::<serde_json::Value>(text).ok()?;
    if !v.get("type")?.as_str()?.starts_with("settings.") {
        return None;
    }
    let field = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_owned();
    Some((field("ns"), field("opId")))
}

/// Applies one settings write for this connection. The success answer (`settings.changed`)
/// reaches this connection through its own settings feed like everyone else's; only an
/// `ifAbsent` reply or a rejection is sent directly. False when the socket is gone.
async fn apply_setting(
    socket: &mut WebSocket,
    settings: &Settings,
    ns: &str,
    op_id: &str,
    op: SettingsOp,
) -> bool {
    // The alerts' runtime state is the evaluator's alone (crate::alerts::model): a client that
    // could write it could re-arm or silence an alert behind the evaluator's back.
    if crate::alerts::server_owned(ns) || crate::trading::server_owned(ns) {
        return send_text(
            socket,
            settings_rejected(
                ns,
                op_id,
                "bad_request",
                "this namespace is written by the server only",
            ),
        )
        .await;
    }
    match settings.apply(ns, op_id, op).await {
        Ok(None) => true,
        Ok(Some(reply)) => send_text(socket, reply).await,
        Err(e) => {
            if e != SettingsError::Unavailable {
                tracing::debug!(ns, code = e.code(), "settings write refused");
            }
            send_text(socket, settings_rejected(ns, op_id, e.code(), e.message())).await
        }
    }
}

/// The connection's quotes opt-in. Dropping it (unsubscribe, or the session ending) releases the
/// market's subscriber count.
struct QuotesSub {
    _guard: QuotesGuard,
    /// `quotes.update`s with `seq` <= this are already in the snapshot.
    seq: u64,
}

/// Sends a fresh `quotes.snapshot` and records its sequence.
async fn send_quotes_snapshot(socket: &mut WebSocket, market: &Market, q: &mut QuotesSub) -> bool {
    let (seq, json) = market.quotes_snapshot();
    q.seq = seq;
    send_text(socket, json).await
}

/// One of the connection's ticker searches (`quotes.search.v1`): the query and what the client
/// was last sent, which the next tick's result is diffed against.
struct SearchSub {
    query: crate::quote_search::Query,
    last: crate::quote_search::Results,
}

/// Sends a fresh `quotes.search.snapshot` of `s` and makes it the diff base. During a rebuild
/// nothing is sent (the `Resync` at its end comes back here).
async fn send_search_snapshot(socket: &mut WebSocket, market: &Market, s: &mut SearchSub) -> bool {
    let Some((t, results)) = market.quote_search(&s.query) else {
        return true;
    };
    let frame = crate::quote_search::snapshot_frame(&s.query.id, t, &results, &market.assets());
    s.last = results;
    send_text(socket, frame).await
}

/// One tick of every search on the connection: an update for each whose result changed.
async fn tick_searches(socket: &mut WebSocket, market: &Market, subs: &mut [SearchSub]) -> bool {
    for s in subs.iter_mut() {
        let Some((t, next)) = market.quote_search(&s.query) else {
            return true;
        };
        let frame =
            crate::quote_search::update_frame(&s.query.id, t, &s.last, &next, &market.assets());
        s.last = next;
        if let Some(f) = frame
            && !send_text(socket, f).await
        {
            return false;
        }
    }
    true
}

/// A fresh snapshot of every search (a seek, a day switch, a lag).
async fn resend_searches(socket: &mut WebSocket, market: &Market, subs: &mut [SearchSub]) -> bool {
    for s in subs.iter_mut() {
        if !send_search_snapshot(socket, market, s).await {
            return false;
        }
    }
    true
}

/// The connection's Halt watcher opt-in (the quotes opt-in's twin).
struct PressureSub {
    _guard: PressureGuard,
    /// `pressure.update`s with `seq` <= this are not newer than the snapshot.
    seq: u64,
}

/// Sends a fresh `pressure.snapshot` and records its sequence.
async fn send_pressure_snapshot(
    socket: &mut WebSocket,
    market: &Market,
    q: &mut PressureSub,
) -> bool {
    let (seq, json) = market.pressure_snapshot();
    q.seq = seq;
    send_text(socket, json).await
}

/// The connection's Ross Panel opt-in (the pressure opt-in's twin).
struct RossSub {
    _guard: RossGuard,
    /// `ross.update`s with `seq` <= this are not newer than the snapshot.
    seq: u64,
}

/// Sends a fresh `ross.snapshot` and records its sequence.
async fn send_ross_snapshot(socket: &mut WebSocket, market: &Market, q: &mut RossSub) -> bool {
    let (seq, json) = market.ross_snapshot();
    q.seq = seq;
    send_text(socket, json).await
}

/// The connection's Favorites opt-in (the Ross opt-in's twin, `crate::favorites`).
struct FavoritesSub {
    _guard: FavoritesGuard,
    /// `favorites.update`s with `seq` <= this are not newer than the snapshot.
    seq: u64,
}

/// Sends a fresh `favorites.snapshot` and records its sequence.
async fn send_favorites_snapshot(
    socket: &mut WebSocket,
    market: &Market,
    q: &mut FavoritesSub,
) -> bool {
    let (seq, json) = market.favorites_snapshot();
    q.seq = seq;
    send_text(socket, json).await
}

/// The connection's movers opt-in (Top Gainers, Bull, Bear; `crate::movers`).
struct MoversSub {
    _guard: MoversGuard,
    /// `movers.update`s with `seq` <= this are not newer than the snapshot.
    seq: u64,
}

/// Sends a fresh `movers.snapshot` and records its generation.
async fn send_movers_snapshot(socket: &mut WebSocket, market: &Market, q: &mut MoversSub) -> bool {
    let (seq, json) = market.movers_snapshot();
    q.seq = seq;
    send_text(socket, json).await
}

/// The connection's candidate-list opt-in (`symbols.v1`, crate::symbols).
struct SymbolsSub {
    _guard: SymbolsGuard,
    /// `symbols.update`s with a `rev` <= this are already in the snapshot.
    rev: u64,
}

/// Sends a fresh `symbols.snapshot` and records its `rev`.
async fn send_symbols_snapshot(
    socket: &mut WebSocket,
    market: &Market,
    q: &mut SymbolsSub,
) -> bool {
    let (rev, json) = market.symbols_snapshot();
    q.rev = rev;
    send_text(socket, json).await
}

/// The connection's notification-log opt-in (`notices.v1`, crate::notices).
struct NoticesSub {
    /// The day shown; appends for any other day are dropped.
    day: chrono::NaiveDate,
    /// Subscribed without a day: follows today into the next ET date.
    follow: bool,
    /// Appends with a `seq` <= this are already in the snapshot.
    seq: u64,
}

/// Sends a fresh `notices.snapshot` of `day` (None = today) and returns what it covers. The
/// snapshot may read files (a past day, the day list), so it is taken off the async workers.
async fn send_notices_snapshot(
    socket: &mut WebSocket,
    feed: &crate::notices::NoticeFeed,
    day: Option<chrono::NaiveDate>,
    follow: bool,
) -> Option<NoticesSub> {
    let feed = feed.clone();
    let snap = tokio::task::spawn_blocking(move || feed.snapshot(day))
        .await
        .ok()?;
    send_text(socket, snap.frame).await.then_some(NoticesSub {
        day: snap.day,
        follow,
        seq: snap.seq,
    })
}

/// The connection's ticker-page opt-in (`insights.ticker.v1`, crate::insights). Only today's has
/// a receiver: a past day is one snapshot.
struct TickerSub {
    rx: Option<tokio::sync::watch::Receiver<Option<std::sync::Arc<String>>>>,
    /// A past day's snapshot being built. Held here rather than sent from the build task, so a
    /// later subscribe (which replaces this) or an unsubscribe drops it: a slow build for the
    /// ticker the user just left can never land after the one they switched to.
    once: Option<tokio::sync::oneshot::Receiver<String>>,
    /// The snapshot went out; later packages are updates.
    sent: bool,
}

/// What the connection's ticker opt-in woke for.
enum TickerWake {
    Changed(bool),
    Once(Option<String>),
}

/// Sends the followed package as a snapshot (first) or an update.
async fn send_ticker(socket: &mut WebSocket, t: &mut TickerSub) -> bool {
    let body = match t.rx.as_mut() {
        Some(rx) => rx.borrow_and_update().clone(),
        None => None,
    };
    let Some(body) = body else {
        return true;
    };
    let kind = if t.sent {
        "insights.ticker.update"
    } else {
        "insights.ticker.snapshot"
    };
    t.sent = true;
    send_text(socket, crate::insights::frame(kind, &body)).await
}

/// The connection's Rocket watch opt-in (the Ross opt-in's twin).
struct RocketSub {
    _guard: RocketGuard,
    /// `rocket.update`s with `seq` <= this are not newer than the snapshot.
    seq: u64,
}

/// Sends a fresh `rocket.snapshot` and records its sequence.
async fn send_rocket_snapshot(socket: &mut WebSocket, market: &Market, q: &mut RocketSub) -> bool {
    let (seq, json) = market.rocket_snapshot();
    q.seq = seq;
    send_text(socket, json).await
}

struct DetectionSub {
    _guard: DetectionGuard,
    epoch: String,
    cursor: u64,
}

async fn send_detection_snapshot(
    socket: &mut WebSocket,
    market: &Market,
    q: &mut DetectionSub,
) -> bool {
    let (epoch, cursor, frame) = market.detection_snapshot();
    q.epoch = epoch;
    q.cursor = cursor;
    send_text(socket, frame).await
}

/// True for a frame whose `type` is in the paper-trading namespace. Such a frame that failed to
/// parse is a bad order (`bad_order`), not a generic `bad_request`, so the client's order ticket
/// can tell its own bug apart from a protocol mismatch.
fn is_paper_command(text: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|v| v.get("type")?.as_str().map(str::to_owned))
        .is_some_and(|t| t.starts_with("order.") || t.starts_with("paper."))
}

/// How many charts one connection may stream at once (the dashboard's multi-chart panel holds 4).
pub const MAX_CHARTS: usize = 4;
/// Longest chart slot id a client may use.
const MAX_CHART_ID: usize = 32;

/// One of the connection's chart slots.
struct Sub {
    /// The slot's name (`""` for the single default slot).
    id: String,
    symbol: String,
    timeframe: Timeframe,
    bin: Price,
    /// Increments with `seq` <= this are already in the snapshot.
    seq: u64,
    /// The slot's server-computed indicators (`indicators.v1`); `None` for a client that sent
    /// no list (it computes its own) and gets no indicator frames.
    studies: Option<IndicatorLease>,
    /// `chart.indicator` frames with `seq` <= this are already in the slot's last
    /// `chart.indicators`. Separate from `seq`: `chart.indicators.set` renews the indicator
    /// snapshot without a candle snapshot. Every slot of one chart key (symbol, timeframe, rth)
    /// on the socket holds the same value: their snapshot is one frame carrying the union of
    /// their studies (`send_indicators`). Were they apart, a frame queued between two slots'
    /// snapshots would go out for the older slot and, carrying every key of the chart, overwrite
    /// the newer slot's values with older ones (frames carry no seq for the client to check).
    ind_seq: u64,
    /// The slot's `chart.levels` demand (`levels.v1`); `None` for a client computing its own.
    levels: Option<ContextLease>,
    /// The slot's `chart.zones` demand (`zones.v1`), on (symbol, timeframe, rth).
    zones: Option<ContextLease>,
    /// Broadcast `chart.levels` / `chart.zones` with a seq <= these are already in the slot's
    /// last levels / zones frame. Every slot of one levels chart (or zones key) on the socket holds
    /// the same value, for the reason `ind_seq` gives: the client routes these frames to every
    /// slot of the chart, so a frame queued before one slot's snapshot must not go out for
    /// another slot after it.
    lvl_seq: u64,
    zone_seq: u64,
}

impl Sub {
    fn levels_key(&self) -> Option<&LevelsKey> {
        match self.levels.as_ref().map(ContextLease::key) {
            Some(ContextKey::Levels(k)) => Some(k),
            _ => None,
        }
    }

    fn zones_key(&self) -> Option<&ChartKey> {
        match self.zones.as_ref().map(ContextLease::key) {
            Some(ContextKey::Zones(k)) => Some(k),
            _ => None,
        }
    }
}

/// One `chart.levels` for `key`; every slot of the socket on it becomes current to it.
async fn send_levels(
    socket: &mut WebSocket,
    market: &Market,
    subs: &mut [Sub],
    key: &LevelsKey,
) -> bool {
    let (seq, json) = market.levels_snapshot(key);
    for s in subs.iter_mut().filter(|s| s.levels_key() == Some(key)) {
        s.lvl_seq = seq;
    }
    send_text(socket, json).await
}

/// One `chart.zones` for `key`; every slot of the socket on it becomes current to it.
async fn send_zones(
    socket: &mut WebSocket,
    market: &Market,
    subs: &mut [Sub],
    key: &ChartKey,
) -> bool {
    let (seq, json) = market.zones_snapshot(key);
    for s in subs.iter_mut().filter(|s| s.zones_key() == Some(key)) {
        s.zone_seq = seq;
    }
    send_text(socket, json).await
}

/// The levels and zones of the slots in `subs` (showing `only`, when given), once per distinct
/// key: after a resync or a load, beside the candle snapshots.
async fn resend_context(
    socket: &mut WebSocket,
    market: &Market,
    subs: &mut [Sub],
    only: Option<&str>,
) -> bool {
    let mut levels: Vec<LevelsKey> = Vec::new();
    let mut zones: Vec<ChartKey> = Vec::new();
    for s in subs
        .iter()
        .filter(|s| only.is_none_or(|sym| s.symbol == sym))
    {
        if let Some(k) = s.levels_key()
            && !levels.contains(k)
        {
            levels.push(k.clone());
        }
        if let Some(k) = s.zones_key()
            && !zones.contains(k)
        {
            zones.push(k.clone());
        }
    }
    for k in &levels {
        if !send_levels(socket, market, subs, k).await {
            return false;
        }
    }
    for k in &zones {
        if !send_zones(socket, market, subs, k).await {
            return false;
        }
    }
    true
}

impl Sub {
    /// Slots with the same key show the same chart, so they share one stream.
    fn same_chart(&self, other: &Sub) -> bool {
        self.symbol == other.symbol && self.timeframe == other.timeframe && self.bin == other.bin
    }

    /// Whether this slot wants the indicator frames of (symbol, timeframe, rth).
    fn wants_indicators(&self, symbol: &str, tf: Timeframe, rth: bool) -> bool {
        self.studies.as_ref().is_some_and(|l| {
            let s = l.studies();
            !s.specs.is_empty()
                && s.chart.rth == rth
                && self.timeframe == tf
                && self.symbol == symbol
        })
    }
}

impl Sub {
    /// The chart key of the slot's studies (also for an empty list, which still gets its empty
    /// `chart.indicators`); `None` for a slot that computes its own.
    fn indicator_chart(&self) -> Option<&ChartKey> {
        self.studies.as_ref().map(|l| &l.studies().chart)
    }
}

/// The union of the studies the slots `subs` hold on `chart` (see `Sub::ind_seq`).
fn chart_studies<'a>(subs: impl IntoIterator<Item = &'a Sub>, chart: &ChartKey) -> Studies {
    Studies::union(
        chart.clone(),
        subs.into_iter()
            .filter_map(|s| s.studies.as_ref().map(IndicatorLease::studies))
            .filter(|st| st.chart == *chart),
    )
}

/// Every slot on `chart` is now current to the `chart.indicators` taken at `seq`.
fn mark_indicators(subs: &mut [Sub], chart: &ChartKey, seq: u64) {
    for s in subs
        .iter_mut()
        .filter(|s| s.indicator_chart() == Some(chart))
    {
        s.ind_seq = seq;
    }
}

/// One `chart.indicators` (no candle snapshot) for `chart`, carrying the studies of every slot
/// on it, which all become current to it.
async fn send_indicators(
    socket: &mut WebSocket,
    market: &Market,
    subs: &mut [Sub],
    chart: &ChartKey,
) -> bool {
    let union = chart_studies(subs.iter(), chart);
    let (seq, json) = market.indicators_snapshot(&union);
    mark_indicators(subs, chart, seq);
    send_text(socket, json).await
}

async fn send_text(socket: &mut WebSocket, text: String) -> bool {
    socket.send(Message::Text(text.into())).await.is_ok()
}

/// A chart error naming its symbol and (for a named slot) the slot, so a client with several
/// charts knows which one it concerns. Same `{type:"error", code, message}` shape as every
/// other error, with two extra fields a single-chart client ignores.
fn chart_error(code: &str, message: &str, symbol: &str, id: &str) -> String {
    let mut v =
        serde_json::json!({ "type": "error", "code": code, "message": message, "symbol": symbol });
    if !id.is_empty() {
        v["id"] = id.into();
    }
    v.to_string()
}

/// A search's refusal, naming the search `id` (absent for the default search).
fn search_error(code: &str, message: &str, id: &str) -> String {
    let mut v = serde_json::json!({ "type": "error", "code": code, "message": message });
    if !id.is_empty() {
        v["id"] = id.into();
    }
    v.to_string()
}

/// Sends a snapshot and the recent tape for `sub`, and records its sequence. With `studies`
/// (the union for the slot's chart key, `chart_studies`), their `chart.indicators` goes right
/// after the candles at the same seq; the caller then marks the chart key's other slots
/// (`mark_indicators`) with the slot's `ind_seq`.
async fn send_snapshot(
    socket: &mut WebSocket,
    market: &Market,
    sub: &mut Sub,
    studies: Option<&Studies>,
) -> bool {
    match market.snapshot_with(&sub.symbol, sub.timeframe, sub.bin, studies) {
        Ok(s) => {
            sub.seq = s.seq;
            if s.indicators.is_some() {
                sub.ind_seq = s.seq;
            }
            if !send_text(socket, s.json).await {
                return false;
            }
            if let Some(json) = s.indicators
                && !send_text(socket, json).await
            {
                return false;
            }
            for row in s.tape {
                if !send_text(socket, row).await {
                    return false;
                }
            }
            true
        }
        Err(SnapshotError::UnknownSymbol) => {
            let frame = chart_error(
                "unknown_symbol",
                "no data for this symbol",
                &sub.symbol,
                &sub.id,
            );
            send_text(socket, frame).await
        }
        Err(SnapshotError::OutsidePriceBand) => {
            let frame = chart_error(
                "outside_price_band",
                "not a candidate: price outside the configured band",
                &sub.symbol,
                &sub.id,
            );
            send_text(socket, frame).await
        }
    }
}

/// A fresh snapshot for every chart held (after a rebuild or a lag), once per distinct chart:
/// slots sharing a chart take the first one's sequence. Likewise one `chart.indicators` per
/// chart key, carrying every slot's studies on it (`Sub::ind_seq`), with the first candle
/// snapshot of that key when there is one.
async fn send_snapshots(socket: &mut WebSocket, market: &Market, subs: &mut [Sub]) -> bool {
    resnapshot(socket, market, subs, None).await
}

/// [`send_snapshots`] for the slots showing `symbol` only.
async fn send_snapshots_of(
    socket: &mut WebSocket,
    market: &Market,
    subs: &mut [Sub],
    symbol: &str,
) -> bool {
    resnapshot(socket, market, subs, Some(symbol)).await
}

async fn resnapshot(
    socket: &mut WebSocket,
    market: &Market,
    subs: &mut [Sub],
    only: Option<&str>,
) -> bool {
    let mut done: Vec<ChartKey> = Vec::new();
    for i in 0..subs.len() {
        if only.is_some_and(|sym| subs[i].symbol != sym) {
            continue;
        }
        // This slot's chart key, unless an earlier slot's snapshot already covered it.
        let chart = subs[i]
            .indicator_chart()
            .filter(|c| !done.contains(c))
            .cloned();
        if let Some(j) = (0..i).find(|&j| subs[j].same_chart(&subs[i])) {
            subs[i].seq = subs[j].seq;
            if let Some(c) = chart {
                if !send_indicators(socket, market, subs, &c).await {
                    return false;
                }
                done.push(c);
            }
            continue;
        }
        let union = chart.as_ref().map(|c| chart_studies(subs.iter(), c));
        if !send_snapshot(socket, market, &mut subs[i], union.as_ref()).await {
            return false;
        }
        if let Some(c) = chart {
            let seq = subs[i].ind_seq;
            mark_indicators(subs, &c, seq);
            done.push(c);
        }
    }
    resend_context(socket, market, subs, only).await
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum ServerMessage<'a> {
    Welcome {
        version: &'a str,
        commit: &'a str,
        #[serde(rename = "heartbeatSecs")]
        heartbeat_secs: u64,
        /// What this server does beyond the base protocol, for a client to opt into
        /// (`indicators.v1`). An older server sends none; a client must treat that as empty.
        features: &'a [&'a str],
    },
    Ping {
        ts: u64,
    },
    Pong {
        ts: u64,
    },
    Error {
        code: &'a str,
        message: &'a str,
    },
}

pub async fn upgrade(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.max_message_size(MAX_MESSAGE_BYTES)
        .on_upgrade(move |socket| session(socket, state))
}

/// An `error` frame with a message built at run time (Alpaca's own rejection text).
fn error_frame(code: &str, message: &str) -> String {
    serde_json::json!({"type": "error", "code": code, "message": message}).to_string()
}

async fn send(socket: &mut WebSocket, msg: &ServerMessage<'_>) -> bool {
    let text = serde_json::to_string(msg).expect("server messages always serialize");
    socket.send(Message::Text(text.into())).await.is_ok()
}

async fn fail(mut socket: WebSocket, code: &str, message: &str, close: u16) {
    let _ = send(&mut socket, &ServerMessage::Error { code, message }).await;
    let _ = socket
        .send(Message::Close(Some(CloseFrame {
            code: close,
            reason: message.to_owned().into(),
        })))
        .await;
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Waits for the next text frame, skipping transport-level ping/pong.
async fn next_text(socket: &mut WebSocket) -> Option<String> {
    while let Some(Ok(msg)) = socket.recv().await {
        match msg {
            Message::Text(t) => return Some(t.to_string()),
            Message::Close(_) => return None,
            _ => {}
        }
    }
    None
}

async fn session(mut socket: WebSocket, state: AppState) {
    let mut shutdown = state.shutdown_signal();
    if *shutdown.borrow() {
        return fail(
            socket,
            "shutting_down",
            "server restarting",
            close_code::AWAY,
        )
        .await;
    }

    let hello = match timeout(HELLO_TIMEOUT, next_text(&mut socket)).await {
        Ok(Some(text)) => serde_json::from_str::<ClientMessage>(&text).ok(),
        Ok(None) => return,
        Err(_) => {
            return fail(
                socket,
                "hello_timeout",
                "expected hello",
                close_code::POLICY,
            )
            .await;
        }
    };
    let Some(ClientMessage::Hello {
        token,
        client,
        source,
    }) = hello
    else {
        return fail(
            socket,
            "bad_request",
            "first message must be hello",
            close_code::POLICY,
        )
        .await;
    };
    if let Some(expected) = state.api_token()
        && !token
            .as_deref()
            .is_some_and(|t| auth::token_matches(expected, t))
    {
        tracing::info!(client = client.as_deref(), "ws hello rejected: bad token");
        // 4401 mirrors HTTP 401 in the application close-code range.
        return fail(socket, "unauthorized", "invalid access token", 4401).await;
    }
    tracing::debug!(client = client.as_deref(), "ws client connected");

    let welcome = ServerMessage::Welcome {
        version: VERSION,
        commit: COMMIT,
        heartbeat_secs: HEARTBEAT.as_secs(),
        features: &[
            crate::indicators::FEATURE,
            crate::indicators::LIMIT_FEATURE,
            crate::chart_context::LEVELS_FEATURE,
            crate::chart_context::ZONES_FEATURE,
            crate::chart_context::SESSIONS_FEATURE,
            crate::alerts::runners::NOTICE_FEATURE,
            crate::symbols::FEATURE,
            crate::notices::FEATURE,
            crate::notices::FEATURE_ROWS,
            crate::insights::FEATURE,
            crate::quote_search::FEATURE,
            crate::time_marks::FEATURE,
            crate::time_marks::NS_FEATURE,
        ],
    };
    // Subscribed before the paper snapshot below, so no change can fall between the two (at
    // worst the client gets the same state twice, which is harmless: it replaces, never merges).
    let market = state.market_for(source).clone();
    let mut feed = market.subscribe();
    if !send(&mut socket, &welcome).await {
        return;
    }
    if !send_text(&mut socket, market.paper_state_frame()).await {
        return;
    }
    if let Some(frame) = market.clock_frame()
        && !send_text(&mut socket, frame).await
    {
        return;
    }
    // Subscribed before the snapshot is taken, for the same reason as the market feed above.
    let settings = state.settings().clone();
    let mut settings_feed = settings.subscribe();
    if !send_text(&mut socket, settings.snapshot_frame().await).await {
        return;
    }
    // Activity and the phone budget (crate::alerts); subscribed before the snapshot, as above.
    let alerts = state.alerts().clone();
    let mut alerts_feed = alerts.subscribe();
    if !send_text(&mut socket, alerts.snapshot_frame().to_string()).await {
        return;
    }
    let mut subs: Vec<Sub> = Vec::new();
    let mut quotes: Option<QuotesSub> = None;
    // Ticker searches (`quotes.search.v1`), each pushed on the connection's own 1 Hz tick, which
    // only runs while one is open.
    let mut searches: Vec<SearchSub> = Vec::new();
    let mut search_tick = tokio::time::interval(crate::market::QUOTES_INTERVAL);
    search_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut pressure: Option<PressureSub> = None;
    let mut ross: Option<RossSub> = None;
    let mut favorites: Option<FavoritesSub> = None;
    let mut movers: Option<MoversSub> = None;
    let mut symbols: Option<SymbolsSub> = None;
    // The Insights log: subscribed to the feed for the whole session (it is quiet), opted in by
    // `notices.subscribe`.
    let notice_feed = state.notice_feed().clone();
    let mut notices_feed = notice_feed.subscribe();
    let mut notices: Option<NoticesSub> = None;
    let mut ticker: Option<TickerSub> = None;
    let mut rocket: Option<RocketSub> = None;
    let mut detection: Option<DetectionSub> = None;
    // Listed among the Runners notification desks while held (`runners.notify`).
    let mut desk: Option<crate::alerts::DeskGuard> = None;

    // Frames for this connection alone from spawned work (a refused Alpaca paper modify/cancel).
    let (direct_tx, mut direct_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    let mut heartbeat = interval_at(Instant::now() + HEARTBEAT, HEARTBEAT);
    let mut last_seen = Instant::now();
    loop {
        tokio::select! {
            Some(frame) = direct_rx.recv() => {
                if !send_text(&mut socket, frame).await {
                    break;
                }
            }
            _ = search_tick.tick(), if !searches.is_empty() => {
                if !tick_searches(&mut socket, &market, &mut searches).await {
                    break;
                }
            }
            _ = heartbeat.tick() => {
                if last_seen.elapsed() > HEARTBEAT * 3 {
                    tracing::debug!("ws client silent for 3 heartbeats, closing");
                    break;
                }
                if !send(&mut socket, &ServerMessage::Ping { ts: now_ms() }).await {
                    break;
                }
            }
            out = feed.recv() => {
                let ok = match out {
                    Ok(o) => match &o.kind {
                        OutKind::Clock(frame) | OutKind::Paper(frame) => {
                            send_text(&mut socket, frame.clone()).await
                        }
                        // Unsubscribed connections drop the frame here without touching it.
                        OutKind::Quotes(frame) => match quotes.as_ref() {
                            Some(q) if o.seq > q.seq => send_text(&mut socket, frame.clone()).await,
                            _ => true,
                        },
                        OutKind::Pressure(frame) => match pressure.as_ref() {
                            Some(q) if o.seq > q.seq => send_text(&mut socket, frame.clone()).await,
                            _ => true,
                        },
                        OutKind::Ross(frame) => match ross.as_ref() {
                            Some(q) if o.seq > q.seq => send_text(&mut socket, frame.clone()).await,
                            _ => true,
                        },
                        OutKind::Favorites(frame) => match favorites.as_ref() {
                            Some(q) if o.seq > q.seq => send_text(&mut socket, frame.clone()).await,
                            _ => true,
                        },
                        OutKind::Movers(frame) => match movers.as_ref() {
                            Some(q) if o.seq > q.seq => send_text(&mut socket, frame.clone()).await,
                            _ => true,
                        },
                        OutKind::Symbols(frame) => match symbols.as_ref() {
                            Some(q) if o.seq > q.rev => send_text(&mut socket, frame.clone()).await,
                            _ => true,
                        },
                        OutKind::Rocket(frame) => match rocket.as_ref() {
                            Some(q) if o.seq > q.seq => send_text(&mut socket, frame.clone()).await,
                            _ => true,
                        },
                        OutKind::RunnersEvent(frame) => match rocket.as_ref() {
                            Some(_) => send_text(&mut socket, frame.clone()).await,
                            None => true,
                        },
                        OutKind::Detection { epoch, cursor, frame } => match detection.as_mut() {
                            Some(q) if market.detection_epoch_is_current(epoch)
                                && (q.epoch != *epoch || *cursor > q.cursor) => {
                                q.epoch = epoch.clone(); q.cursor = *cursor;
                                send_text(&mut socket, frame.clone()).await
                            }
                            _ => true,
                        },
                        OutKind::Resync => {
                            (match quotes.as_mut() {
                                Some(q) => send_quotes_snapshot(&mut socket, &market, q).await,
                                None => true,
                            }) && (match pressure.as_mut() {
                                Some(q) => send_pressure_snapshot(&mut socket, &market, q).await,
                                None => true,
                            }) && (match ross.as_mut() {
                                Some(q) => send_ross_snapshot(&mut socket, &market, q).await,
                                None => true,
                            }) && (match favorites.as_mut() {
                                Some(q) => send_favorites_snapshot(&mut socket, &market, q).await,
                                None => true,
                            }) && (match movers.as_mut() {
                                Some(q) => send_movers_snapshot(&mut socket, &market, q).await,
                                None => true,
                            }) && (match symbols.as_mut() {
                                Some(q) => send_symbols_snapshot(&mut socket, &market, q).await,
                                None => true,
                            }) && (match rocket.as_mut() {
                                Some(q) => send_rocket_snapshot(&mut socket, &market, q).await,
                                None => true,
                            }) && (match detection.as_mut() {
                                Some(q) => send_detection_snapshot(&mut socket, &market, q).await,
                                None => true,
                            }) && resend_searches(&mut socket, &market, &mut searches).await
                                && send_snapshots(&mut socket, &market, &mut subs).await
                        }
                        // Live: a symbol's archived history just went into the engine; a slot
                        // showing it had an empty chart (crate::history).
                        OutKind::Loaded(symbol) => {
                            send_snapshots_of(&mut socket, &market, &mut subs, symbol).await
                        }
                        // Indicator values: once per connection, if any slot on this chart key holds
                        // studies and has not already got them in its `chart.indicators`. Keys a
                        // slot did not ask for ride along; the client ignores them.
                        OutKind::Indicator { symbol, timeframe, rth, json } => {
                            if !subs.iter().any(|s| s.wants_indicators(symbol, *timeframe, *rth) && o.seq > s.ind_seq) {
                                true
                            } else if json.starts_with(crate::indicators::SNAPSHOT_PREFIX)
                                && let chart = (ChartKey { symbol: symbol.clone(), timeframe: *timeframe, rth: *rth })
                                && chart_studies(subs.iter(), &chart).limit.is_some()
                            {
                                // A deep revision's snapshot carries every bar; this socket asked
                                // for fewer (`limit`), so it gets its own, as on a resync.
                                send_indicators(&mut socket, &market, &mut subs, &chart).await
                            } else {
                                send_text(&mut socket, json.clone()).await
                            }
                        }
                        // Levels and zones: once per connection, if a slot on that chart asked and
                        // has not already got this state in its own snapshot.
                        OutKind::Levels { key, json } => {
                            if subs.iter().any(|s| s.levels_key() == Some(key) && o.seq > s.lvl_seq) {
                                send_text(&mut socket, json.clone()).await
                            } else {
                                true
                            }
                        }
                        OutKind::Zones { key, json } => {
                            if subs.iter().any(|s| s.zones_key() == Some(key) && o.seq > s.zone_seq) {
                                send_text(&mut socket, json.clone()).await
                            } else {
                                true
                            }
                        }
                        // Most frames are for symbols no slot shows: dropped here untouched.
                        OutKind::Chart { symbol, .. } if !subs.iter().any(|s| s.symbol == *symbol) => true,
                        // The stock traded out of the price band: it is no longer a candidate,
                        // so every slot showing it stops here and each is told why.
                        OutKind::Chart { symbol, .. } if !market.is_candidate(symbol) => {
                            let gone: Vec<String> = subs.iter().filter(|s| s.symbol == *symbol).map(|s| s.id.clone()).collect();
                            subs.retain(|s| s.symbol != *symbol);
                            let mut ok = true;
                            for id in gone {
                                let frame = chart_error(
                                    "outside_price_band",
                                    "no longer a candidate: price left the configured band",
                                    symbol,
                                    &id,
                                );
                                if !send_text(&mut socket, frame).await {
                                    ok = false;
                                    break;
                                }
                            }
                            ok
                        }
                        // A tape, halt or LULD row: once per symbol, whatever the slots' timeframes.
                        OutKind::Chart { symbol, timeframe: None, json, .. } => {
                            if subs.iter().any(|s| s.symbol == *symbol && o.seq > s.seq) {
                                send_text(&mut socket, json.clone()).await
                            } else {
                                true
                            }
                        }
                        // A candle frame: once per distinct bin among the slots on this chart
                        // that have not already got it in their snapshot, with the profile's
                        // figures for that bin (Market::chart_frame).
                        OutKind::Chart { symbol, timeframe: Some(tf), update, json } => {
                            let mut bins: Vec<Price> = Vec::new();
                            for s in subs.iter().filter(|s| s.symbol == *symbol && s.timeframe == *tf && o.seq > s.seq) {
                                if !bins.contains(&s.bin) {
                                    bins.push(s.bin);
                                }
                            }
                            let mut ok = true;
                            for bin in bins {
                                if !send_text(&mut socket, market.chart_frame(&o, update, json, bin)).await {
                                    ok = false;
                                    break;
                                }
                            }
                            ok
                        }
                    },
                    Err(RecvError::Lagged(n)) => {
                        tracing::debug!(skipped = n, "ws client lagged; resyncing with a snapshot");
                        send_text(&mut socket, market.paper_state_frame()).await
                            && match quotes.as_mut() {
                                Some(q) => send_quotes_snapshot(&mut socket, &market, q).await,
                                None => true,
                            }
                            && match pressure.as_mut() {
                                Some(q) => send_pressure_snapshot(&mut socket, &market, q).await,
                                None => true,
                            }
                            && match ross.as_mut() {
                                Some(q) => send_ross_snapshot(&mut socket, &market, q).await,
                                None => true,
                            }
                            && match favorites.as_mut() {
                                Some(q) => send_favorites_snapshot(&mut socket, &market, q).await,
                                None => true,
                            }
                            && match movers.as_mut() {
                                Some(q) => send_movers_snapshot(&mut socket, &market, q).await,
                                None => true,
                            }
                            && match symbols.as_mut() {
                                Some(q) => send_symbols_snapshot(&mut socket, &market, q).await,
                                None => true,
                            }
                            && match rocket.as_mut() {
                                Some(q) => send_rocket_snapshot(&mut socket, &market, q).await,
                                None => true,
                            }
                            && match detection.as_mut() {
                                Some(q) => send_detection_snapshot(&mut socket, &market, q).await,
                                None => true,
                            }
                            && resend_searches(&mut socket, &market, &mut searches).await
                            && send_snapshots(&mut socket, &market, &mut subs).await
                    }
                    Err(RecvError::Closed) => true,
                };
                if !ok {
                    break;
                }
            }
            changed = settings_feed.recv() => {
                let ok = match changed {
                    Ok(frame) => send_text(&mut socket, frame.to_string()).await,
                    Err(RecvError::Lagged(n)) => {
                        tracing::debug!(skipped = n, "ws client lagged on settings; resending the snapshot");
                        send_text(&mut socket, settings.snapshot_frame().await).await
                    }
                    // The store (and its sender) lives as long as the AppState; if it ever went,
                    // recv would return at once forever, so end the session rather than spin.
                    Err(RecvError::Closed) => false,
                };
                if !ok {
                    break;
                }
            }
            wake = async {
                match ticker.as_mut() {
                    Some(TickerSub { once: Some(o), .. }) => TickerWake::Once(o.await.ok()),
                    Some(TickerSub { rx: Some(rx), .. }) => TickerWake::Changed(rx.changed().await.is_ok()),
                    _ => std::future::pending().await,
                }
            } => {
                let ok = match (wake, ticker.as_mut()) {
                    (TickerWake::Changed(true), Some(t)) => send_ticker(&mut socket, t).await,
                    (TickerWake::Once(Some(frame)), Some(_)) => {
                        ticker = None;
                        send_text(&mut socket, frame).await
                    }
                    _ => {
                        ticker = None;
                        true
                    }
                };
                if !ok {
                    break;
                }
            }
            append = notices_feed.recv() => {
                let ok = match (append, notices.as_mut()) {
                    (_, None) => true,
                    (Ok(a), Some(n)) if a.seq <= n.seq => true,
                    // A rollover to the day shown (subscribed to it by date just before midnight)
                    // has nothing to add.
                    (Ok(a), Some(n)) if a.day == n.day => {
                        n.seq = a.seq;
                        a.roll || send_text(&mut socket, a.frame.clone()).await
                    }
                    // Today rolled over (midnight, `roll`, or the new day's first line): a
                    // following subscription moves to the new day with its snapshot (which holds
                    // that line); a fixed day ignores other days.
                    (Ok(a), Some(n)) if n.follow && a.day > n.day => {
                        match send_notices_snapshot(&mut socket, &notice_feed, Some(a.day), true).await {
                            Some(sub) => {
                                notices = Some(sub);
                                true
                            }
                            None => false,
                        }
                    }
                    (Ok(_), Some(_)) => true,
                    (Err(RecvError::Lagged(_)), Some(n)) => {
                        // A following subscription resyncs to today, not to the day it held: the
                        // lines it skipped may have been the next day's first, which is what would
                        // have moved it there, and no later line of the old day would ever do it.
                        let follow = n.follow;
                        let day = if follow { None } else { Some(n.day) };
                        match send_notices_snapshot(&mut socket, &notice_feed, day, follow).await {
                            Some(sub) => {
                                notices = Some(sub);
                                true
                            }
                            None => false,
                        }
                    }
                    // The feed lives as long as the state; if it ever went, end rather than spin.
                    (Err(RecvError::Closed), Some(_)) => false,
                };
                if !ok {
                    break;
                }
            }
            frame = alerts_feed.recv() => {
                let ok = match frame {
                    Ok(f) => send_text(&mut socket, f.to_string()).await,
                    Err(RecvError::Lagged(n)) => {
                        tracing::debug!(skipped = n, "ws client lagged on alerts; resending Activity");
                        send_text(&mut socket, alerts.snapshot_frame().to_string()).await
                    }
                    // An idle service (tests) never sends; its sender lives as long as the state.
                    Err(RecvError::Closed) => false,
                };
                if !ok {
                    break;
                }
            }
            _ = shutdown.changed() => {
                fail(socket, "shutting_down", "server restarting", close_code::AWAY).await;
                return;
            }
            msg = socket.recv() => {
                let Some(Ok(msg)) = msg else { break };
                last_seen = Instant::now();
                let Message::Text(text) = msg else {
                    if matches!(msg, Message::Close(_)) { break }
                    continue;
                };
                let reply = match serde_json::from_str::<ClientMessage>(&text) {
                    Ok(ClientMessage::Ping { ts }) => Some(ServerMessage::Pong { ts }),
                    Ok(ClientMessage::Pong { .. }) => None,
                    Ok(ClientMessage::ChartSubscribe { symbol, timeframe, profile_bin, id, indicators, rth, levels, zones, limit }) => {
                        let id = id.unwrap_or_default();
                        let held = subs.iter().position(|s| s.id == id);
                        let symbol = symbol.trim().to_ascii_uppercase();
                        let studies = crate::indicators::parse_request(&symbol, timeframe, indicators.as_ref(), rth)
                            .map(|st| st.map(|st| st.with_limit(limit)));
                        if id.chars().count() > MAX_CHART_ID {
                            Some(ServerMessage::Error {
                                code: "bad_request",
                                message: "chart id too long",
                            })
                        } else if held.is_none() && subs.len() >= MAX_CHARTS {
                            Some(ServerMessage::Error {
                                code: "too_many_charts",
                                message: "at most 4 charts per connection; unsubscribe one first",
                            })
                        } else if studies.is_err() {
                            let frame = chart_error("bad_request", "indicators must be a list", &symbol, &id);
                            if !send_text(&mut socket, frame).await {
                                break;
                            }
                            None
                        } else {
                            match api::parse_bin(profile_bin, market.default_bin()) {
                                Ok(bin) => {
                                    let mut s = Sub {
                                        id,
                                        symbol: symbol.clone(),
                                        timeframe,
                                        bin,
                                        seq: 0,
                                        // Counted before the snapshot builds the series, so a
                                        // concurrent release elsewhere cannot drop them between.
                                        studies: studies.ok().flatten().map(|st| market.indicators_acquire(st)),
                                        ind_seq: 0,
                                        levels: (levels == Some(true)).then(|| market.context_acquire(ContextKey::Levels(LevelsKey { symbol: symbol.clone(), timeframe }))),
                                        zones: (zones == Some(true)).then(|| market.context_acquire(ContextKey::Zones(ChartKey { symbol: symbol.clone(), timeframe, rth: rth.unwrap_or(false) }))),
                                        lvl_seq: 0,
                                        zone_seq: 0,
                                    };
                                    // With the other slots' studies on the same chart key (not
                                    // the one this replaces): see `Sub::ind_seq`.
                                    let chart = s.indicator_chart().cloned();
                                    let union = chart.as_ref().map(|c| {
                                        let others = subs.iter().enumerate().filter(|(i, _)| Some(*i) != held).map(|(_, o)| o);
                                        chart_studies(others.chain([&s]), c)
                                    });
                                    if !send_snapshot(&mut socket, &market, &mut s, union.as_ref()).await {
                                        break;
                                    }
                                    if let Some(c) = &chart {
                                        mark_indicators(&mut subs, c, s.ind_seq);
                                    }
                                    // The client replaced its copy of this chart with the new
                                    // snapshot, so every slot sharing it is current to its seq.
                                    for other in subs.iter_mut().filter(|o| o.same_chart(&s)) {
                                        other.seq = s.seq;
                                    }
                                    let i = match held {
                                        Some(i) => {
                                            subs[i] = s;
                                            i
                                        }
                                        None => {
                                            subs.push(s);
                                            subs.len() - 1
                                        }
                                    };
                                    // After the candles: the levels and zones of this chart, which
                                    // every other slot on the same key takes too (`Sub::lvl_seq`).
                                    if let Some(k) = subs[i].levels_key().cloned()
                                        && !send_levels(&mut socket, &market, &mut subs, &k).await
                                    {
                                        break;
                                    }
                                    if let Some(k) = subs[i].zones_key().cloned()
                                        && !send_zones(&mut socket, &market, &mut subs, &k).await
                                    {
                                        break;
                                    }
                                    None
                                }
                                Err(_) => Some(ServerMessage::Error {
                                    code: "bad_request",
                                    message: "invalid profileBin",
                                }),
                            }
                        }
                    }
                    Ok(ClientMessage::ChartIndicatorsSet { id, indicators, rth, zones, limit }) => {
                        let id = id.unwrap_or_default();
                        match subs.iter().position(|s| s.id == id) {
                            None => {
                                let frame = chart_error("bad_request", "no chart in this slot", "", &id);
                                if !send_text(&mut socket, frame).await {
                                    break;
                                }
                            }
                            Some(i) => {
                                let s = &mut subs[i];
                                match crate::indicators::parse_request(&s.symbol, s.timeframe, Some(&indicators), rth) {
                                    Ok(st) => {
                                        let st = st.map(|st| st.with_limit(limit));
                                        // The new lease is taken before the old one goes, so the
                                        // studies both lists share keep their series.
                                        let lease = st.map(|st| market.indicators_acquire(st));
                                        s.studies = lease;
                                        // The zones follow the layer (`zones`, absent = as it was)
                                        // and the slot's rth: a new key takes a new lease (taken
                                        // before the old one goes) and answers with its zones.
                                        let on = zones.unwrap_or(s.zones.is_some());
                                        let want = on.then(|| ChartKey { symbol: s.symbol.clone(), timeframe: s.timeframe, rth: rth.unwrap_or(false) });
                                        let zones_changed = want.as_ref() != s.zones_key();
                                        if zones_changed {
                                            s.zones = want.map(|k| market.context_acquire(ContextKey::Zones(k)));
                                        }
                                        let zkey = s.zones_key().cloned().filter(|_| zones_changed);
                                        let chart = s.indicator_chart().cloned();
                                        if let Some(c) = chart
                                            && !send_indicators(&mut socket, &market, &mut subs, &c).await
                                        {
                                            break;
                                        }
                                        if let Some(k) = zkey
                                            && !send_zones(&mut socket, &market, &mut subs, &k).await
                                        {
                                            break;
                                        }
                                    }
                                    Err(m) => {
                                        let frame = chart_error("bad_request", m, &s.symbol, &id);
                                        if !send_text(&mut socket, frame).await {
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                        None
                    }
                    Ok(ClientMessage::ChartUnsubscribe { id }) => {
                        let id = id.unwrap_or_default();
                        subs.retain(|s| s.id != id);
                        None
                    }
                    Ok(ClientMessage::ReplayControl { action, value }) => {
                        match market.control(&action, value.as_ref()).await {
                            Ok(_) => None,
                            Err(ControlError::NotReplay) => Some(ServerMessage::Error {
                                code: "not_replay",
                                message: "replay controls are only available in replay mode",
                            }),
                            Err(ControlError::BadRequest(m)) => {
                                tracing::debug!(error = %m, "bad replay.control");
                                Some(ServerMessage::Error {
                                    code: "bad_request",
                                    message: "invalid replay.control",
                                })
                            }
                            Err(ControlError::UnknownDay(d)) => {
                                tracing::debug!(day = %d, "replay.control day: not in the archive");
                                Some(ServerMessage::Error {
                                    code: "unknown_day",
                                    message: "that day is not in the replay archive",
                                })
                            }
                            Err(ControlError::Rejected(m)) => {
                                tracing::warn!(error = %m, "replay.control refused by the driver");
                                Some(ServerMessage::Error {
                                    code: "day_unavailable",
                                    message: "that day could not be opened; the replay did not change",
                                })
                            }
                            Err(ControlError::Failed(_)) => Some(ServerMessage::Error {
                                code: "replay_unavailable",
                                message: "replay is not running",
                            }),
                        }
                    }
                    // The live market routed to Alpaca paper (crate::trading): the HTTP call runs
                    // in its own task so this socket keeps flowing; a place answers through the
                    // broadcast state, a refused modify/cancel through `direct`.
                    Ok(ClientMessage::OrderPlace(cmd)) => {
                        match market.paper_route() {
                            Some(route) => {
                                tokio::spawn(async move { route.place(cmd).await });
                            }
                            None => {
                                market.paper_place(cmd);
                            }
                        }
                        None
                    }
                    Ok(ClientMessage::OrderModify(cmd)) => match market.paper_route() {
                        Some(route) => {
                            let tx = direct_tx.clone();
                            tokio::spawn(async move {
                                if let Err(e) = route.modify(cmd).await {
                                    let _ = tx.send(error_frame(e.code, &e.message));
                                }
                            });
                            None
                        }
                        None => match market.paper_modify(cmd) {
                            Ok(()) => None,
                            Err(e) => Some(ServerMessage::Error { code: e.code(), message: e.message() }),
                        },
                    },
                    Ok(ClientMessage::OrderCancel(cmd)) => match market.paper_route() {
                        Some(route) => {
                            let tx = direct_tx.clone();
                            tokio::spawn(async move {
                                if let Err(e) = route.cancel(&cmd.id).await {
                                    let _ = tx.send(error_frame(e.code, &e.message));
                                }
                            });
                            None
                        }
                        None => match market.paper_cancel(&cmd.id) {
                            Ok(()) => None,
                            Err(e) => Some(ServerMessage::Error { code: e.code(), message: e.message() }),
                        },
                    },
                    Ok(ClientMessage::PaperReset) => {
                        if market.paper_route().is_some() {
                            Some(ServerMessage::Error {
                                code: "not_supported",
                                message: "reset is for the simulator only; an Alpaca paper account is reset from Alpaca's dashboard",
                            })
                        } else {
                            market.paper_reset();
                            None
                        }
                    }
                    Ok(ClientMessage::QuotesSubscribe) => {
                        let q = quotes.get_or_insert_with(|| QuotesSub {
                            _guard: market.quotes_subscribe(),
                            seq: 0,
                        });
                        if !send_quotes_snapshot(&mut socket, &market, q).await {
                            break;
                        }
                        None
                    }
                    Ok(ClientMessage::QuotesUnsubscribe) => {
                        quotes = None;
                        None
                    }
                    Ok(ClientMessage::QuotesSearch(req)) => {
                        let id = req.id.clone().unwrap_or_default();
                        match crate::quote_search::Query::parse(req) {
                            Err(message) => {
                                if !send_text(&mut socket, search_error("bad_request", &message, &id)).await {
                                    break;
                                }
                                None
                            }
                            Ok(query) => match searches.iter().position(|s| s.query.id == query.id) {
                                // A new query for a held id replaces it with a fresh snapshot (the
                                // client re-ranks on a query, filter or limit change).
                                Some(i) => {
                                    searches[i].query = query;
                                    if !send_search_snapshot(&mut socket, &market, &mut searches[i]).await {
                                        break;
                                    }
                                    None
                                }
                                None if searches.len() >= crate::quote_search::MAX_SEARCHES => {
                                    let frame = search_error(
                                        "too_many_searches",
                                        "this connection already holds the most searches it may",
                                        &id,
                                    );
                                    if !send_text(&mut socket, frame).await {
                                        break;
                                    }
                                    None
                                }
                                None => {
                                    searches.push(SearchSub { query, last: Default::default() });
                                    let n = searches.len() - 1;
                                    if !send_search_snapshot(&mut socket, &market, &mut searches[n]).await {
                                        break;
                                    }
                                    None
                                }
                            },
                        }
                    }
                    Ok(ClientMessage::QuotesSearchStop { id }) => {
                        let id = id.unwrap_or_default();
                        searches.retain(|s| s.query.id != id);
                        None
                    }
                    Ok(ClientMessage::QuotesDirectory) => {
                        if !send_text(&mut socket, market.quote_directory()).await {
                            break;
                        }
                        None
                    }
                    Ok(ClientMessage::PressureSubscribe) => {
                        let q = pressure.get_or_insert_with(|| PressureSub {
                            _guard: market.pressure_subscribe(),
                            seq: 0,
                        });
                        if !send_pressure_snapshot(&mut socket, &market, q).await {
                            break;
                        }
                        None
                    }
                    Ok(ClientMessage::PressureUnsubscribe) => {
                        pressure = None;
                        None
                    }
                    Ok(ClientMessage::RossSubscribe) => {
                        let q = ross.get_or_insert_with(|| RossSub {
                            _guard: market.ross_subscribe(),
                            seq: 0,
                        });
                        if !send_ross_snapshot(&mut socket, &market, q).await {
                            break;
                        }
                        None
                    }
                    Ok(ClientMessage::RossUnsubscribe) => {
                        ross = None;
                        None
                    }
                    Ok(ClientMessage::FavoritesSubscribe) => {
                        let q = favorites.get_or_insert_with(|| FavoritesSub {
                            _guard: market.favorites_subscribe(),
                            seq: 0,
                        });
                        if !send_favorites_snapshot(&mut socket, &market, q).await {
                            break;
                        }
                        None
                    }
                    Ok(ClientMessage::FavoritesUnsubscribe) => {
                        favorites = None;
                        None
                    }
                    Ok(ClientMessage::MoversSubscribe) => {
                        let q = movers.get_or_insert_with(|| MoversSub {
                            _guard: market.movers_subscribe(),
                            seq: 0,
                        });
                        if !send_movers_snapshot(&mut socket, &market, q).await {
                            break;
                        }
                        None
                    }
                    Ok(ClientMessage::MoversUnsubscribe) => {
                        movers = None;
                        None
                    }
                    Ok(ClientMessage::SymbolsSubscribe) => {
                        let q = symbols.get_or_insert_with(|| SymbolsSub {
                            _guard: market.symbols_subscribe(),
                            rev: 0,
                        });
                        if !send_symbols_snapshot(&mut socket, &market, q).await {
                            break;
                        }
                        None
                    }
                    Ok(ClientMessage::SymbolsUnsubscribe) => {
                        symbols = None;
                        None
                    }
                    Ok(ClientMessage::NoticesSubscribe { day }) => {
                        // Parsed into a date before it names a file, as in GET /api/notices.
                        let parsed = match day.as_deref().filter(|d| !d.is_empty()) {
                            Some(d) => chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").map(Some),
                            None => Ok(None),
                        };
                        match parsed {
                            Ok(day) => {
                                match send_notices_snapshot(&mut socket, &notice_feed, day, day.is_none()).await {
                                    Some(sub) => notices = Some(sub),
                                    None => break,
                                }
                                None
                            }
                            Err(_) => Some(ServerMessage::Error {
                                code: "bad_request",
                                message: "day must be YYYY-MM-DD",
                            }),
                        }
                    }
                    Ok(ClientMessage::NoticesUnsubscribe) => {
                        notices = None;
                        None
                    }
                    Ok(ClientMessage::InsightsTickerSubscribe { symbol, day }) => {
                        match crate::insights::parse_request(&symbol, day.as_deref()) {
                            Ok((symbol, day)) => {
                                let insights = state.insights().clone();
                                let now = now_ms() as i64 * 1_000_000;
                                if crate::insights::is_running(day, now) {
                                    // Shared with every connection on the same symbol-day; its
                                    // current package (if built) goes out now, later ones as
                                    // they change.
                                    let mut t = TickerSub {
                                        rx: Some(insights.follow(symbol, day)),
                                        once: None,
                                        sent: false,
                                    };
                                    let ready = t.rx.as_ref().is_some_and(|rx| rx.borrow().is_some());
                                    if ready && !send_ticker(&mut socket, &mut t).await {
                                        break;
                                    }
                                    ticker = Some(t);
                                } else {
                                    let (tx, once) = tokio::sync::oneshot::channel();
                                    ticker = Some(TickerSub { rx: None, once: Some(once), sent: true });
                                    tokio::spawn(async move {
                                        let v = insights.package(symbol, day).await;
                                        let _ = tx.send(crate::insights::frame(
                                            "insights.ticker.snapshot",
                                            &v.to_string(),
                                        ));
                                    });
                                }
                                None
                            }
                            Err(why) => Some(ServerMessage::Error {
                                code: "bad_request",
                                message: why,
                            }),
                        }
                    }
                    Ok(ClientMessage::InsightsTickerUnsubscribe) => {
                        ticker = None;
                        None
                    }
                    Ok(ClientMessage::RocketSubscribe) => {
                        let q = rocket.get_or_insert_with(|| RocketSub {
                            _guard: market.rocket_subscribe(),
                            seq: 0,
                        });
                        if !send_rocket_snapshot(&mut socket, &market, q).await {
                            break;
                        }
                        None
                    }
                    Ok(ClientMessage::RocketUnsubscribe) => {
                        rocket = None;
                        None
                    }
                    Ok(ClientMessage::DetectionSubscribe) => {
                        let q = detection.get_or_insert_with(|| DetectionSub {
                            _guard: market.detection_subscribe(), epoch: String::new(), cursor: 0,
                        });
                        if !send_detection_snapshot(&mut socket, &market, q).await { break; }
                        None
                    }
                    Ok(ClientMessage::DetectionUnsubscribe) => { detection = None; None }
                    Ok(ClientMessage::ContextGet { symbol }) => {
                        let frame = market.context_decision(&symbol.trim().to_ascii_uppercase());
                        if !send_text(&mut socket, frame).await {
                            break;
                        }
                        None
                    }
                    Ok(ClientMessage::SettingsSet { ns, value, op_id, if_absent }) => {
                        let op = SettingsOp::Set { value, if_absent };
                        if !apply_setting(&mut socket, &settings, &ns, &op_id, op).await {
                            break;
                        }
                        None
                    }
                    Ok(ClientMessage::SettingsPatch { ns, set, remove, op_id }) => {
                        let op = SettingsOp::Patch { set, remove };
                        if !apply_setting(&mut socket, &settings, &ns, &op_id, op).await {
                            break;
                        }
                        None
                    }
                    Ok(ClientMessage::PushRegister { token, platform, name, runners_new, runners_breakout, runners_burst, runners_running, runners_run, runners_fast, more }) => {
                        let runners = more.with(crate::alerts::push::RunnersOptIn { new: runners_new, breakout: runners_breakout, burst: runners_burst, running: runners_running, run: runners_run, fast: runners_fast, ..Default::default() });
                        if alerts.register_device(token.trim(), &platform, &name, runners) {
                            None
                        } else {
                            Some(ServerMessage::Error { code: "bad_request", message: "not an Expo push token" })
                        }
                    }
                    Ok(ClientMessage::PushUnregister { token }) => {
                        alerts.unregister_device(token.trim());
                        None
                    }
                    Ok(ClientMessage::RunnersNotify { enabled, new, breakout, burst, running, run, fast, more }) => {
                        if enabled {
                            let kinds = crate::alerts::DeskKinds::from_opt_in(more.with(crate::alerts::push::RunnersOptIn { new, breakout, burst, running, run, fast, ..Default::default() }));
                            match &desk {
                                Some(g) => g.set(kinds),
                                None => desk = Some(alerts.desk(kinds)),
                            }
                        } else {
                            desk = None;
                        }
                        None
                    }
                    Err(_) if let Some((ns, op_id)) = settings_frame_ids(&text) => {
                        let frame = settings_rejected(&ns, &op_id, "bad_request", "malformed settings message");
                        if !send_text(&mut socket, frame).await {
                            break;
                        }
                        None
                    }
                    Err(_) if is_paper_command(&text) => Some(ServerMessage::Error {
                        code: "bad_order",
                        message: "malformed or unknown order command",
                    }),
                    Ok(ClientMessage::Hello { .. }) | Err(_) => Some(ServerMessage::Error {
                        code: "bad_request",
                        message: "unexpected message",
                    }),
                };
                if let Some(reply) = reply
                    && !send(&mut socket, &reply).await
                {
                    break;
                }
            }
        }
    }
}
