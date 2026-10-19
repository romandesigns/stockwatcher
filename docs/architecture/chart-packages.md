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
  https://claude.ai/artifact/R5VpkCELmhv4srLeuEuv7b): the server's top 6 symbols by one-sided
  buying or selling pressure in the last minute, as gauge cards (a 240° speedometer left; ticker,
  price and change right). Direction is the hue (green bull, red bear) and strength the lightness;
  a halted card keeps its last pressure on a grey arc and shows "Halted ~HH:MM" in amber (a LULD
  pause's expected reopening; another halt shows when it began). Pressing a card loads its ticker
  into the selected chart. Web: a strip above the charts inside the panel's scroll (`HaltWatcher`,
  three columns wide, two narrower, one on a phone, by container queries). Phone: the region under
  the charts, a `Segmented` "Halt watcher | Most active" (one column folded, two unfolded by
  `maxChartsFor`'s sw600dp rule). Data: `pressure.subscribe` on the dashboard's own socket, so it
  follows Settings → Data like the charts. The gauge is the shared `GAUGE` geometry and colours in
  `@stockwatcher/shared` `pressure.ts`, drawn by `PressureGauge` on each platform, and is wavykit's
  `pressure-gauge`.
  - **Pressure** (server, `crates/server/src/pressure.rs`, trades only: the feed has no quotes):
    over the last 60 s of OHLC-counted prints (the engine's verdict, so the candles' condition
    policy), sides by the tick rule (uptick buy, downtick sell, zero tick inherits).
    `buyShare = B / (B + S)`; side bull when ≥ 0.5. `I = min(1, |2·buyShare − 1| / 0.6)` (80/20 is
    fully one-sided). `pace = V / A`, `A` the symbol's average volume per minute so far in the
    session (premarket, regular, after hours each on its own; at least one minute).
    `P = min(1, ln(1 + pace) / ln 11)`. **`pressure = round(100 · I · (0.4 + 0.6 · P))`**. No score
    below 10 prints or 2,000 shares in the window. Frozen at a halt until the resume.
  - **Ranking**: in the server's price band; members stay down to half those minimums; an outsider
    replaces the weakest unhalted member only by 10 points, and a card moves above its neighbour
    only by 10 points; halted members stay. Ranked once per second (the quotes task, off the
    ingest path); one pre-serialized `pressure.update` with the whole board goes to subscribers
    when it changed.

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
| Indicators | SMA, EMA, session VWAP, Bollinger, RSI (Wilder), MACD; at most 3 overlays + 2 panes (colour-validated slots) | `indicators.ts` |
| Drawings | trend, ray, horizontal, vertical, rectangle, Long and Short position plans; a measure tool (not stored; also Shift+drag on the web); per-symbol undo/redo; colours | `drawings.ts`, `position.ts`, `toolbox.ts` |
| Levels (layers) | session volume profile, halts, LULD bands, premarket high/low, day range, open, opening range, previous close, fills | `levels.ts` |
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

- `ChartFeed` (`apps/web/src/feed/types.ts`) has two implementations, the server feed and the
  in-browser fixture, so nothing above it knows which is active. The same feed also carries the paper
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
