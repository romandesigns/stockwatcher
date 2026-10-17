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
| Keyboard | ←/→ (Shift ×10), Home/End/Esc, +/−/0, **A** = auto-scale price, **Shift+F10** = price axis menu | Ours is richer for chart navigation; Legend's shortcuts are for trading and layouts |
| Drawing tools, indicators, chart trading | ✗ | Out of scope for spec 01 (no strategies or trading this phase) |

Scale changes are announced through the chart's polite live region ("Price axis: Logarithmic.", "Price axis
scaled manually…", "Price axis auto-scaled."); hover and scrub are not announced.

**Not done, or not possible in a browser:**
- Haptic feedback on entering scrub (no reliable web API; `navigator.vibrate` is absent on iOS and not a tick).
- Pan momentum (fling). Kept out deliberately; it needs a per-frame decay driven by the renderer.
- Vertical price panning by touch: a vertical finger move belongs to the page scroll, so manual-range price panning
  is mouse-only for now.
- The React Native mapping of these inputs (responder or Gesture Handler events onto `GestureInput`).

## Sources
- [Using advanced charts | Robinhood](https://robinhood.com/us/en/support/articles/using-advanced-charts)
- [Using charts | Robinhood](https://robinhood.com/support/articles/360001325643/using-charts)
- [Widgets in Robinhood Legend | Robinhood](https://robinhood.com/us/en/support/articles/widgets-in-robinhood-legend/)
- [Trading with Robinhood Legend | Robinhood](https://robinhood.com/us/en/support/articles/trading-with-robinhood-legend)
- [Introducing Robinhood Legend Charts on Mobile | Robinhood Newsroom](https://robinhood.com/us/en/newsroom/introducing-robinhood-legend-charts-on-mobile/)
- [Chart indicators on Legend | Robinhood](https://robinhood.com/us/en/support/articles/chart-indicators-on-legend/) (found, not read in detail)
