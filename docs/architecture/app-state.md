# App state: one Zustand store per client

Status: built, 2026-10-18. Owner request: "We want to set up global state in all clients", with
Zustand (chosen over Redux Toolkit) and "sync everything possible" across devices.

## Shape

Every client (web, desktop = the web app in Tauri, phone) creates **one** store from
`@stockwatcher/app-state` (`packages/app-state`) and reads it with a one-line `useApp(selector)`
hook (zustand's `useStore`). The store is built on `zustand/vanilla`, so the package has no React
dependency, and its shape and rules exist once instead of twice (they were hand-copied
between `App.tsx` and `ChartScreen.tsx` before).

| Piece | Location |
|---|---|
| Store, actions, slices | `packages/app-state/src/store.ts` (`createAppStore`) |
| App-level synced namespaces + validators | `packages/app-state/src/namespaces.ts` (`APP_NS`, `parseView`, …) |
| Chart synced namespaces + validators | `packages/chart-core/src/syncedSettings.ts` (`SYNC_NS`), unchanged |
| Web / desktop | `apps/web/src/state/store.ts` (`getAppStore`, `useApp`), used by `App.tsx` |
| Phone | `apps/mobile/src/state/store.ts` (`loadAppStore`, `useApp`, `appActions`), used by `ChartScreen.tsx` |

## Three kinds of state, one owner each

| Kind | What | Owner | Persisted |
|---|---|---|---|
| **Synced** | indicators, levels, chart settings, drawing settings, drawings (per symbol), **ticker, timeframe, profile bin** (`view`), **trade size** (`tradeQty`), **search recents** (`searchRecents`), **the dashboard panel** (`dashboard`: chart count 1–4 and each slot's ticker and timeframe) | the settings sync engine (`settings-sync.md`); the store mirrors it | server + the engine's local cache |
| **Device** | toolbox place, pane heights, drawing tools shown | phone: the store's `device` slice, saved to AsyncStorage under the old `chart.*` keys. Web: `CandleChart` under its `persistKey` (`sw-chart:*`) | this device |
| **Session** | open sheets and dialogs, the inspected candle, the symbol actually shown, **the selected dashboard chart** (`dashboardSelected`) | the store | no |

Screens that show several symbols at once (the dashboard) read each one's drawings through
`store.drawingsFor(symbol)`: a subscribe/get/set handle on the engine's memoized view, written back
as a per-drawing patch like `setDrawings`.

Not in the store: market data (series, tape, clock, quotes, paper state). chart-core's
`ChartStore` and the feed hooks already hold it as external stores, and the replay clock (day,
speed, play/pause) is the server's, so every client already shares it.

## Rules that are easy to break

- **Synced setters write to the engine, never to the store.** The store follows the engine's
  change event, so a local edit and another device's edit take the same path and the store can
  never disagree with what will be sent. Only the slice whose namespace changed is re-read.
- **The price-band fallback is not written back.** When the synced ticker is outside the price
  band, a client shows its first candidate instead (`followCandidates` on the web, the same
  rule on the phone) but leaves the synced ticker alone: a device with a momentarily stale
  candidate list must not move every other device's chart. Drawings follow the symbol actually
  shown (`showSymbol`, called in a layout effect so a switch never flashes the old marks).
- **A ticker or timeframe change clears the inspected candle**, whichever device made it.
- **Select small.** `useApp((s) => s.layers)` re-renders only when layers change; a selector that
  builds a new object re-renders on every store change (use zustand's `useShallow` if one is
  really needed). Read actions once (`useState(() => store.getState())`); they never change.
- **New synced value:** add the namespace and a parser in `namespaces.ts` (or `syncedSettings.ts`
  for chart models), mirror it in `createAppStore` (initial read, setter, the subscribe switch),
  and add a test. The server needs no change: it stores any namespace opaquely.

## Migration

Recents and the trade size lived per device before (`sw-search:recent` / `search.recent`,
`sw-chart:tradeQty`). On first start with the store, each is seeded `ifAbsent` into its empty
namespace, so the first device to connect makes its value the shared one, the same rule as
`migrateLegacySettings`. The old keys are left in place and no longer read once the engine has a
value. The ticker, timeframe and bin were session state before, so there was nothing to migrate.
