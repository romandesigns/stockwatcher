# Robinhood chart gestures (research, 2026-10-17)

This note covers what users can do with Robinhood's charts on mobile and web/desktop (Robinhood Legend). It
exists to inform the Stockwatcher chart. Unless marked otherwise, each item comes from Robinhood's own help
center or newsroom (sources at the end). **Unverified** marks common knowledge that the documentation does
not state; confirm those on a device.

Robinhood has three chart surfaces:
1. The **simple chart**: the line or candle chart on a stock's page, in the app and on the web.
2. **Advanced charts**: the mobile app's full chart, the same engine as Legend on mobile since June 2025.
3. **Robinhood Legend**: the desktop/web trading workspace with chart widgets.

## Mobile (app)

### Simple chart
| Gesture | Effect | Source |
|---|---|---|
| Tap a time span (1D … MAX for stocks and ETFs; 1H … 5Y for crypto) | Changes the range | Using charts |
| Scroll the time-span row to its end, then tap the gear | Chart settings: Line ⇄ Candlestick, "Show all hours" (extended hours) | Using charts |
| Press and drag along the line | Scrubs through time; the header shows the price at that point | **Unverified** (well known, not in the help pages) |

### Advanced chart (Legend engine)
| Gesture | Effect | Source |
|---|---|---|
| **Pinch** | Zooms in and out of time | Using advanced charts; Legend-on-mobile post |
| **Swipe left/right** | Pans earlier or later in time | Using advanced charts |
| **Long press** in the chart | **Scrubs**: shows detailed data for the selected point in time | Using advanced charts |
| **Drag the y-axis** up or down | Rescales the price axis | Using advanced charts |
| **Double-tap** | Returns to an auto-scaled y-axis | Using advanced charts |
| Long press to scrub, then tap **+** on the price axis | Places a limit or stop order at that price | Using advanced charts |
| **Press and drag an open order** | Modifies it | Using advanced charts |
| Tap **X** next to an open order | Cancels it | Using advanced charts |
| Tap **Buy / Sell MKT** | Market order (optional auto-send skips confirmation) | Using advanced charts |
| **+** → Drawings | Choose a drawing, then position, resize, color, thickness and style | Using advanced charts |
| **+** → indicators | Swipe a list of common ones or "View all"; tap to add (max 10), tap again to hide; gear to customize or remove; "Save as set" | Using advanced charts |
| ⋯ chart settings | Show or hide indicators, drawings, orders and positions; chart type; trading settings; visual markers; y-axis scaling; extended hours | Using advanced charts |
| Tap the symbol | Search for another symbol | Using advanced charts |

The data runs down to **2-tick live candles**, with 80+ indicators and web/mobile charts synced automatically
(Legend-on-mobile post). Landscape and fullscreen are **not documented**.

## Web / desktop

### Simple chart (robinhood.com)
| Interaction | Effect | Source |
|---|---|---|
| Chart icon → Line / Candlesticks | Chart type | Using advanced charts (web) |
| Gear → "Show extended hours" | Extended hours | Using advanced charts (web) |
| Time span, then interval | Custom intervals | Using advanced charts (web) |
| "Advanced" or the expand icon | Opens the advanced view (indicators persist across symbols) | Using advanced charts (web) |
| Hover over the chart | Shows the price and time under the cursor | **Unverified** |

### Robinhood Legend chart widget
| Interaction | Effect | Source |
|---|---|---|
| **Mouse wheel / trackpad** | Zooms the x-axis | Widgets in Legend |
| **Preset times** (lower left) | Jump to a range | Widgets in Legend |
| **Interval** arrows (bottom) | Tick, Range, Seconds, Minute, Hour, Day, Week, Month, Year, or custom | Widgets in Legend |
| **Right-click the y-axis** | Auto-scale price axis; fit all indicators, orders or positions; Linear (price), Linear (%) or Logarithmic; options price type (mid, bid, last, ask) | Widgets in Legend |
| Drag the baseline line | Moves the baseline chart's reference | Widgets in Legend |
| Chart-types icon | Area, Bar, Baseline, Candle, Equivolume, Heikin-Ashi, Hollow candles, Line, Scatter, Trend | Widgets in Legend |
| Drawing icons (top; widen the widget if they're hidden) | Rays, vertical lines, shapes, arrows, callouts, highlighter, path, pencil; **Sync on/off** mirrors drawings onto other charts of the same symbol | Widgets in Legend |
| ⋮ → Settings → Data | Extended hours; show open orders or positions | Widgets in Legend |
| **Right-click any chart point**, or **+** on the right | Order at that price: above the market a buy stop or sell limit, below it a buy limit or sell stop | Trading with Legend |
| Click an **order pill** | Fills the order form (cancel or replace) | Trading with Legend |
| **Drag a position or order pill** | Updates a limit or stop order (not for crypto) | Trading with Legend |
| Drag to pan | Not documented (**unverified**; common in charting libraries) | — |

### Legend keyboard shortcuts (they act on the active widget)
| Action | Mac | Windows |
|---|---|---|
| Shortcuts menu | ⌘ / | Ctrl / |
| Switch layout | ⌥ ←/→ | Alt ←/→ |
| Jump to layout 1–8 | ⌥ 1–8 | Alt 1–8 |
| New layout | ⌥ N | Alt N |
| Search symbol | / | / |
| Select row | ↑/↓ | ↑/↓ |
| Select widget | ⇧ arrow | Shift arrow |
| Buy market / Sell market | ⇧B / ⇧S | Shift B / Shift S |
| Buy limit at ask / Sell limit at bid | ⇧⌥B / ⇧⌥S | Shift Alt B / Shift Alt S |
| Edit quantity | ⇧Q | Shift Q |
| Flatten position | ⇧⌥F | Shift Alt F |

With auto-send off, a shortcut opens a prefilled order form; with auto-send on, it submits.

## Compared with Stockwatcher's chart (`packages/chart-web`, after the gesture work of 2026-10-17)

The gestures are recognized by a pure state machine in `chart-core` (`src/gestures.ts`, `GestureController`),
which takes normalized pointer input and returns actions, so the web and the future React Native renderer share
one set of rules and thresholds. Price scale types and the auto/manual price range live in `chart-core` too
(`scale.ts`, `Viewport.price`, `ChartState.priceScaleType`).

| Robinhood | Stockwatcher now | Note |
|---|---|---|
| Pinch to zoom (mobile) | ✓ Pinch zooms **time only** around the fingers' midpoint; the midpoint's travel pans | Parity. Price is never pinched, as in Robinhood |
| Swipe to pan (mobile) | ✓ A horizontal swipe pans; a mostly vertical one is released so the page scrolls (`touch-action: pan-y`) | Parity. Direction is decided once the finger leaves an 8 px slop. No momentum (inertia) yet |
| Long press to scrub (mobile) | ✓ Long press (450 ms) scrubs: the inspector and crosshair follow the finger, clamped to the plot; lifting returns to the latest candle | Parity, without haptics (the web has no usable haptics; React Native can add them on entering scrub) |
| Tap | Tap pins a candle; tapping it again returns to the latest | Ours. The pin waits out the 300 ms double-tap window so a double tap never pins first |
| Drag the y-axis to rescale (mobile) | ✓ Drag the price axis (touch or mouse): up zooms in, down zooms out, around the plot's centre; the range becomes **manual** | Parity. The axis drag claims the touch, so it never scrolls the page |
| Double-tap to auto-scale (mobile) | ✓ Double tap (300 ms, 24 px) in the plot or on the price axis auto-scales; on the time axis it follows the latest candle | Parity |
| Wheel zoom (desktop) | ✓ Zooms time around the pointer; over the price axis it scales price; Ctrl+wheel (trackpad pinch) zooms time | Parity, plus price-axis wheel |
| Drag to pan (desktop) | ✓ Drag pans time, and also price while the range is manual | Parity |
| Hover readout | ✓ Crosshair and inspector | Parity |
| Double-click (desktop) | ✓ Plot or price axis: auto-scale price. Time axis: follow the latest candle | Changed: double-click in the plot used to follow latest (still on the time axis, `0` and `End`) |
| Right-click y-axis: auto-scale, Linear (price), Linear (%), Logarithmic | ✓ Right-click (or long-press, or Shift+F10 / the Menu key) the price axis opens `PriceAxisMenu`: auto-scale and the three scale types | Parity for these four items. Legend's "fit indicators/orders/positions" and options price type do not apply (none of those exist here) |
| Scale types | ✓ Linear; Linear (%) from the first visible candle's open (moves as you pan); Logarithmic (1-2-5 × 10ⁿ ticks once the range spans 2×) | Hit testing, the profile overlay, the crosshair label and the last-price tag all use the same mapping |
| Preset ranges and interval picker | Timeframe selector 15s/30s/1m/5m | Ours fits the spec; preset ranges could come later |
| Halts and LULD bands | ✓ A halt is shaded on the chart with its reason and length ("HALT LUDP · 1m 30s"), with a HALTED badge while one is on; LULD bands draw as stepped dashed lines, and an off-screen band is pinned to the edge as a tag | Owner priority (small caps halt constantly). Bands are **modelled** from the Tier 2 rules (docs/architecture/halts-luld-fills.md), a guide, not the exchange's numbers |
| Key levels | ✓ Premarket high/low, high/low of day, open, previous close, labelled; off-screen levels pinned to the edge with arrows. Each layer switches on or off in the Levels menu | Computed on the client from the candles |
| Fills and trade list | ✓ Buy/sell arrows at each simulated fill's time and price; the Trades list shows fills with realized P&L | From paper.state fills (last 200) |
| Measure | ✓ Ruler tool, or Shift+drag on the web: change, %, bars and time between two points; not stored | |
| Undo / redo | ✓ Palette buttons; Ctrl+Z, Ctrl+Shift+Z / Ctrl+Y on the web; per symbol, one step per committed change | |
| Right-click to trade (practice) | ✓ Right-click anywhere in the plot opens the trade menu at the cursor, priced at that height on the tick grid, with the crosshair held on it: one-click buy/sell at that price (the resting type Legend's rule gives), market buy/sell, a remembered share size (100/500/1,000), the full ticket at that price, or a long/short position plan dropped there | Owner request 2026-10-17. Simulated only. Touch keeps long press for scrubbing; on the phone the price axis long press offers "Order at this price…" |
| Candle countdown | ✓ Time left in the forming candle ("0:42") under the last-price tag on the price axis, on the chart's clock (replay time in replay); hidden when no candle is forming | Owner request 2026-10-17 (TradingView's spot for it) |
| Drawing tools on/off | ✓ A pencil switch at the left of the toolbar shows or hides the floating drawing tools (persisted); hiding returns to the cursor | Owner request 2026-10-17 |
| Movable drawing tools | ✓ The drawing tools float over the chart as a palette: drag it by its grip (or focus the grip and use the arrow keys), flip it between a column and a row, and it stays where it was left (stored as fractions of the room it can move in, so it survives resizes). It also holds the selected drawing's colour (8 swatches) and delete | Owner request 2026-10-17, as TradingView's floating drawing toolbar. Position tools keep their green/red |
| Back to latest | ✓ A round →\| button at the plot's bottom-right whenever the view is panned into the past; it slides home with an ease-out (220–520 ms, instant with reduced motion) and then follows new candles. A touch on the chart stops the slide. `0` does the same; double-clicking the time axis still snaps | Owner request 2026-10-17 (TradingView has the same button). Web and phone; a light haptic on the phone |
| Keyboard | ←/→ (Shift ×10), Home/End/Esc, +/−/0, **A** = auto-scale price, **Shift+F10** = price axis menu | Ours is richer for chart navigation; Legend's shortcuts are for trading and layouts |
| Drawing tools | ✓ Trend line, ray, horizontal line (with its price tag), vertical line, range (rotatable: a selected range has a rotate handle above its top edge; snaps near 15° steps and shows its angle). Press-drag-release, or tap then tap; drag a handle or the body to edit; Delete removes | Chart-owned (`chart-core/src/drawings.ts`); anchors are time and price, so drawings survive pans, zooms and timeframe switches. Kept per symbol |
| Indicators (max 10) | ✓ VWAP, EMA 9/20, SMA 50, Bollinger 20/2 on the chart (up to 3); RSI 14, MACD below (up to 2) | Fewer than Robinhood's 10 on purpose: three overlay colours validate all-pairs for colour-blind readers, so a fourth would be indistinguishable |
| Chart trading: + on the price axis, drag an order, ✕ to cancel | ✓ **Simulated**: "+" at the hover price (web) or "Order at this price…" in the price axis menu (long press on touch), a Trade button, drag a pill to modify (one modify on release), ✕ to cancel; position line with unrealized P&L | Fills only in our simulator (`docs/architecture/chart-trading-sim.md`); every surface says "Simulated" |

Scale changes are announced through the chart's polite live region ("Price axis: Logarithmic.", "Price axis
scaled manually…", "Price axis auto-scaled."); hover and scrub are not announced.

**Phone only (chart-native, 2026-10-17):** what the browser cannot do is done in the React Native chart,
which feeds Gesture Handler touches into the same `GestureController` with the phone's options on:
- **Haptics** (expo-haptics): a light impact on entering a scrub, a selection tick per candle while
  scrubbing, a medium bump when a long press opens the price axis menu, a light bump on grabbing an order
  or a drawing and on placing a drawing, and a success notification when one of the symbol's simulated
  orders fills.
- **Momentum:** a touch pan released faster than 0.25 px/ms keeps going and decays with a 325 ms time
  constant (close to iOS scrolling); a press during the fling stops it and is not a tap. The web chart
  has it too.
- **Vertical price panning:** the phone's chart screen does not scroll, so a mostly vertical swipe pans
  price and makes the range manual; a double tap returns to auto. A mostly horizontal swipe stays
  time-only while the range is auto, so jitter never knocks it out of auto. On the web a vertical swipe
  still scrolls the page unless the range is already manual (`touchPricePan: "manual"`).

## Sources
- [Using advanced charts | Robinhood](https://robinhood.com/us/en/support/articles/using-advanced-charts)
- [Using charts | Robinhood](https://robinhood.com/support/articles/360001325643/using-charts)
- [Widgets in Robinhood Legend | Robinhood](https://robinhood.com/us/en/support/articles/widgets-in-robinhood-legend/)
- [Trading with Robinhood Legend | Robinhood](https://robinhood.com/us/en/support/articles/trading-with-robinhood-legend)
- [Introducing Robinhood Legend Charts on Mobile | Robinhood Newsroom](https://robinhood.com/us/en/newsroom/introducing-robinhood-legend-charts-on-mobile/)
- [Chart indicators on Legend | Robinhood](https://robinhood.com/us/en/support/articles/chart-indicators-on-legend/) (found, not read in detail)
