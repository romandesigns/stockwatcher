# Settings sync across clients

Status: built, 2026-10-18. Owner request: "We want to synchronize all the clients, so any setting
change takes place in all clients. Basically, if I change one indicator on the desktop/web, it
should change everywhere else live and the change should persist."

Decided scope: the user's **settings and drawings** sync; each device keeps its **own view**. It is
a single-user app (one API token), so every connected client (web, desktop = the web app in Tauri,
phone) is the same person.

**Widened 2026-10-18** with the app store (`app-state.md`): the owner asked to "sync everything
possible", so the ticker, timeframe and profile bin (`view`), the trade size (`tradeQty`) and the
search recents (`searchRecents`) sync too. Each client reads all of it through its Zustand store
(`@stockwatcher/app-state`), which replaced the two `useSynced.ts` hooks.

## Where things live

| Piece | Location |
|---|---|
| Store, limits, persistence | `crates/server/src/settings.rs` |
| Wire protocol | top of `crates/server/src/ws.rs`, mirrored in `packages/shared/src/protocol.ts` |
| Client engine (cache, queue, migration, throttle) | `packages/shared/src/settingsSync.ts`, attached by `createWsClient({settings})` in `ws.ts` |
| Namespaces, validation, drawing map, migration rule | `packages/chart-core/src/syncedSettings.ts` |
| App-level namespaces (`view`, `tradeQty`, `searchRecents`) | `packages/app-state/src/namespaces.ts` |
| Web / desktop | `apps/web/src/settings/sync.ts` (the engine), read through `apps/web/src/state/store.ts` |
| Phone | `apps/mobile/src/settings/sync.ts` (the engine), read through `apps/mobile/src/state/store.ts` |
| Two-client check against the real server | `scripts/settings-sync-e2e.ts` (by hand, not in CI) |

## Synced vs device-local

| Setting | Before (web / phone key) | Now | Why |
|---|---|---|---|
| Indicators, their parameters (colours follow their order) | `sw-chart:indicators` / `chart.indicators` | **synced** `indicators` | a preference of the person |
| The settings of indicators turned off (style, periods, markers), restored when turned back on; "Reset to default" clears them (2026-11-07) | none (new) | **synced** `indicatorMemory` (map id → spec, entry patches) | a preference of the person |
| Indicator line colours, **global**: one colour per line on every chart (main, dashboard cells, ticker page) and device (2026-11-07) | on each spec (`colors`, 2026-11-07 only; migrated once) | **synced** `indicatorColors` (flat map `"<identity>/<role>"` → `#rrggbb`, one entry per role, entry patches) | owner: "the colors change should propagate globally and not independently per chart"; see *Indicator colours* below |
| The Insights ticker page chart's own indicators | `stockwatcher:insights-ticker:indicators` / `insights-ticker:indicators` (seeded once) | **synced** `tickerIndicators` | a preference of the person |
| Levels / context layers (halts, LULD, key levels, opening range, fills, profile) | `sw-chart:layers` / `chart.layers` | **synced** `layers` | same |
| Chart settings (type, price scale, sessions, volume, grid, shading, breaks, high/low, auto stretch, zoom preset, visibility) | `sw-chart:settings` / `chart.settings` | **synced** `chartSettings` | same; a remote change does not re-zoom this device's view (below) |
| Drawing toolbox contents (Settings → Drawing) | `sw-chart:drawing` / `chart.drawing` | **synced** `drawingSettings` | same |
| Drawings (incl. colour, position tools) | `sw-chart:drawings` / `chart.drawings` (all symbols in one value) | **synced** `drawings:<SYMBOL>`, per drawing | the user's marks on a chart, wanted everywhere |
| Ticker, timeframe, profile bin | session state | **synced** `view` (since 2026-10-18) | owner: "sync everything possible"; a ticker outside the price band falls back per device without writing back (`app-state.md`) |
| Zoom / pan / manual price range / auto-stretch override | session state | device-local | same |
| Toolbox position and orientation | `sw-chart:toolbox` / `chart.toolbox` | device-local | depends on the screen: a column on the left of a desktop is a row on a phone |
| Oscillator pane heights | `sw-chart:panes` / `chart.paneHeights` | device-local | pixels, so they depend on the screen's height |
| Drawing tools shown / hidden | `sw-chart:toolsVisible` / `chart.toolsVisible` | device-local | a screen-space choice (a phone may hide them, a desktop not) |
| Trade menu share size | `sw-chart:tradeQty` (web only) | **synced** `tradeQty` (since 2026-10-18) | owner: "sync everything possible"; one list of sizes (chart-core `TRADE_SIZES`) |
| Settings dialog tab, open dialogs/menus, selected drawing | `sw-settings:tab`, session state | device-local | UI state |
| Ticker search recents | `sw-search:recent` / `search.recent` | **synced** `searchRecents` (since 2026-10-18) | owner: "sync everything possible"; seeded once from the old keys |
| Dashboard panel: chart count, each chart's ticker and timeframe | (new) | **synced** `dashboard` (since 2026-10-18) | the main page follows the person; an empty or out-of-band slot shows a candidate per device without writing back |
| Price alerts (rules) and their phone preferences | (new) | **synced** `alerts` (per alert, patched like drawings) and `alerts.prefs` (since 2026-10-19) | every device sees and edits the same alerts; the server reads them to evaluate (`app-state.md` "Alerts") |
| Alert states (armed, cooling, done, expired) | (new) | **synced, server-written** `alerts.state` | the evaluator's verdict; client writes are refused (`bad_request`) |
| Alert Activity, registered phones | (new) | server only (`DATA_DIR/alerts`), sent on connect (`alerts.history`) | grows with the market, not with edits; a push token is per device and never synced |
| Selected dashboard chart | (new) | device-local (session) | a pointer on this screen, like a selected drawing |
| Access token | `sw-access-token` / keystore | device-local, never synced | a credential |

When in doubt about a per-screen layout value it stays local: syncing a pixel height or a corner
position from a 27" monitor to a phone would be wrong on the phone.

A remote `chartSettings` change applies at once (chart type, grid, …) but does not re-zoom or
re-fit the current view on the other devices (`settingsFromRemote` on both `CandleChart`s): a
zoom preset chosen on the phone must not throw away the desktop's current zoom and pan. The next
ticker or timeframe opened there uses the new preset.

## Indicator colours

Owner, 2026-11-07: an indicator line's colour is one preference, not a per-chart setting. Code:
`packages/chart-core/src/indicatorColors.ts` (model, keys, migration rule), applied in
`packages/app-state/src/store.ts`; the pickers call the store's `indicatorColorActions` through
each chart's `indicatorColorActions` prop.

- **Key:** `"<identity>/<role>"`. The identity is the indicator type, plus the period where the
  period makes a different line: `ema:9`, `ema:20`, `sma:50`. VWAP has no period, and Bollinger,
  RSI and MACD are one study each in the catalogue (their periods tune the same study), so they key
  as `vwap`, `bb`, `rsi`, `macd`. The role is chart-core's `ColorRole` (`line`, `upper`, `fill`,
  `macd`, `signal`, `histUp`, `upSoft`, `area`, …). Example: `{"ema:9/line": "#aa00ff",
  "macd/histUp": "#3fb97a"}`.
- **Writes:** `settings.patch`, one entry per role, so two devices recolouring different lines (or
  different parts of one MACD) both land; a role set back to its default is removed. "Reset to
  default" in an indicator's settings removes every role of that identity (also those of MACD
  styles not shown) and, as before, resets style, markers and periods of that chart's spec.
- **Reads:** the store applies the map to every list it hands out (`indicators`,
  `tickerIndicators`, each dashboard slot's own list), replacing the per-spec `colors`, so the
  charts and menus keep reading `spec.colors` and need no knowledge of the map. Lists are written
  without colours.
- **Migration:** until the namespace exists the per-spec colours still show. The first build that
  knows it seeds it once (`ifAbsent`) from every per-spec colour it holds (memory, dashboard
  slots, ticker page, main chart; later wins, the main chart last since there are no per-spec
  timestamps); the first device to connect wins, the others adopt the server's map. Afterwards
  per-spec colours are ignored and dropped the next time each list is written. Nothing about the
  older namespaces' shapes changed, so an older build still loads everything; it shows its own
  per-spec colours until it updates.

## Server store

- A namespace (`[A-Za-z0-9._:-]`, 1–80 chars) holds one opaque JSON value, a `rev` (1, 2, 3, …
  per namespace) and `updatedAt` (wall-clock ms). The server never interprets values: the
  clients own the schemas and validate everything they read (chart-core parsers), so a new chart
  setting never needs a server release.
- **Files:** one per namespace, `DATA_DIR/settings/<namespace in hex>.json`, each holding
  `{ns, rev, updatedAt, value}`. One file per namespace because writes are per namespace and
  frequent (a drawing drop rewrites one symbol's small file, not every symbol's drawings) and a
  failed write can only affect its own namespace. Hex names because namespaces contain `:` (not
  allowed on Windows) and may differ only in case. Writes are temp + fsync + rename (+ directory
  fsync on Unix), and a write is on disk **before** it is applied and broadcast. A leftover `.tmp`
  is deleted at startup; an unreadable file is renamed `.corrupt` and skipped.
- **Limits:** 256 KiB per document, 8 MiB in total, 2000 namespaces, keys of a map ≤ 128 chars.
  A whole-value `set` is also bounded by the socket's 64 KiB message limit (the client refuses to
  send anything over 60 000 chars itself). Refusals: `too_large`, `store_full`, `bad_request`,
  `not_a_map`; a failed disk write is `settings_unavailable` and nothing changes.
- Independent of the market: replay seeks and day switches, live mode and the archive never touch
  it, and it is not part of the archive (the archiver only manages `DATA_DIR/archive`).

### Concurrency: last writer wins, by arrival

Every write is applied in the order it reaches the server, the namespace's `rev` goes up by one,
and the result is broadcast to **every** connection, the writer included. No compare-and-set:
with one user, two writes to one namespace are the same person on two devices, and the later
arrival is their latest action. A CAS would refuse it and make the client rebase and retry,
which can only lose or delay what the user just did. Nothing writes in response to a broadcast,
so nothing ping-pongs.

### Drawings: per drawing, not per list

`drawings:<SYMBOL>` is a map `drawingId → {z, d}` (`d` the drawing, `z` its paint/hit order,
since the server keeps object keys sorted), written with `settings.patch` (set/remove entries).
With last-writer-wins on a symbol's whole list, two devices editing *different* drawings at the
same moment would lose one; with entry patches both land, and only two edits of the *same*
drawing resolve last-writer-wins. Clients diff their new list against the map
(`diffDrawings`), so a drop sends only the drawing that moved. Drawings are small, so the
snapshot carries every symbol's drawings at once rather than per chart subscription.

## Protocol (after `welcome`)

```jsonc
// server → client, once after welcome (after paper.state and the clock), and to a lagged connection
{ "type": "settings.snapshot", "docs": [ { "ns": "layers", "rev": 3, "updatedAt": 1792348297784, "value": { … } } ] }
// client → server
{ "type": "settings.set",   "ns": "indicators", "value": [ … ], "opId": "k3x9-…-7" }
{ "type": "settings.set",   "ns": "layers", "value": { … }, "opId": "…", "ifAbsent": true }    // migration seed
{ "type": "settings.patch", "ns": "drawings:SMPL", "set": { "trend-…": { "z": 4, "d": { … } } }, "remove": ["hline-…"], "opId": "…" }
// server → every connection after an applied write (full value, also after a patch)
{ "type": "settings.changed", "ns": "drawings:SMPL", "rev": 12, "updatedAt": …, "value": { … }, "opId": "…" }
// server → the writer only; nothing changed
{ "type": "settings.rejected", "ns": "…", "opId": "…", "code": "too_large", "message": "…" }
```

An `ifAbsent` set on a namespace that exists answers the writer alone with a `settings.changed`
carrying the current value, its unchanged rev and the writer's opId. `GET /api/settings` (bearer
token) returns the snapshot, for debugging. Every server frame starts with `{"type":`, which the
feeds use to route settings frames without parsing them as chart frames.

## Client engine

Per namespace the client keeps what the server last **confirmed** and a queue of its own
**pending** edits, and the UI shows the confirmed value with the pending edits folded on top.

- **Optimistic:** an edit shows at once and is cached (localStorage `sw-sync:v1`, phone preference
  `sync.v1`), so the app starts with the last known settings before the socket connects.
- **Echo suppression:** our own `settings.changed` (same opId) just leaves the queue; the shown
  value does not change, nothing re-renders (shown values compare as canonical JSON, since the
  server sorts keys).
- **No lost latest action, no flicker:** another device's change arriving while our edit is in
  flight updates the confirmed value underneath, our edit stays on top, and the server applies ours
  after it; when our echo arrives both sides agree.
- **Sending:** a throttle with a trailing flush rather than a pure debounce. The first change in
  a namespace goes out at once (a toggle reaches the other devices immediately); further changes
  within 200 ms are coalesced into one write at the end of the window, so a burst sends the final
  value in about two writes (measured: 30 rapid changes → 2 server revs). `pagehide` flushes on
  the web.
- **Offline:** edits made with no connection are coalesced (one per namespace), persisted, and
  sent after the next snapshot, last writer wins. Edits in flight when the socket drops are sent
  again after the reconnect; every operation is idempotent, so a duplicate costs one rev, never
  data. While the server is unreachable the page shows "Server unreachable" and edits wait in the
  cache.
- **Never trusts remote JSON:** every value, local or remote, goes through the app's own parser
  (`parseIndicators`, `parseLayers`, `parseChartSettings`, `parseDrawingSettings`,
  `drawingsFromDoc`), so junk from an older or newer build falls back to defaults field by field.

### Migration of existing local settings

On the first run with sync (no `sw-sync:v1` / `sync.v1` cache yet), each client reads its old keys
(table above) once and queues every value that was ever saved, validated, as an `ifAbsent`
seed (`migrateLegacySettings`, one rule for both apps). After the first snapshot:

- the server has no value for that namespace → the seed is sent; **the first client to connect
  wins**, so the owner's current desktop/web setup becomes the shared one;
- the server has one → the seed is dropped and the server's value replaces the local one;
- two clients racing to seed → the server applies the first and answers the second with the
  shared value, which it adopts.

The old keys are left in place and never read again. A server whose `settings/` directory was
wiped is re-seeded the same way from what each client last knew.

## Deploy notes

Nothing to configure. `DATA_DIR` is `/app/data` in the image, the named volume
`stockwatcher_data` in `infra/vps/docker-compose.yml`, owned by the image's non-root user, so the
server creates `settings/` on first start and it survives restarts and redeploys. The rootfs is
read-only; nothing else is written. Back it up with the volume.
