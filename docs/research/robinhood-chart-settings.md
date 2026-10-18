# Robinhood chart settings (research, 2026-10-18)

What Robinhood offers as chart settings, and what Stockwatcher's chart settings (first a Chart button left of Indicators, since 2026-10-18 the Settings dialog's Chart tab) build
from it. It continues `robinhood-chart-gestures.md`; sources are listed at the end. Items marked
**unverified** come from a source that could not be read in full.

## What Robinhood documents

| Surface | Setting | Source |
|---|---|---|
| Simple chart (app and web), gear | View type: Line or Candlestick; "Show all hours" on/off (stocks and ETFs) | Using charts |
| Web chart | Chart icon: Line / Candlesticks; gear: "Show extended hours" | Using advanced charts |
| Advanced chart (app), ⋯ chart settings | Show or hide all indicators, drawings, open orders and open positions; chart type; trading settings (auto-send, session, time in force); visual markers for the symbol and indicators; y-axis scaling; extended-hours data | Using advanced charts |
| Advanced chart (app) | Drag the y-axis to scale it; double-tap returns to an auto-scaled y-axis | Using advanced charts |
| Legend chart widget, chart-types icon | Area, Bar, Baseline, Candle, Equivolume, Heikin-Ashi, Hollow candles, Line, Scatter, Trend | Widgets in Legend |
| Legend, right-click the y-axis | Auto-scale price axis; fit all indicators / orders / positions; Linear (price), Linear (percent, "percentage change from the first visible candle"), Logarithmic; options price type | Widgets in Legend |
| Legend, ⋮ → Settings → Data | Show extended hours; show extended-hours orders, open orders, open positions | Widgets in Legend |
| Legend (2025 post) | Highlight pre-market and extended-hours data; display the high and low price; adjust zoom settings | Robinhood on X (**unverified**: the post could not be fetched; its text is from the search index) |
| Legend auras | Background coloured by trading session (pre-market, market hours, extended hours, overnight, closed), portfolio or BTC performance, or "Focus" | Auras on Legend |

Documented colour rules (Widgets in Legend):
- **Hollow candles**: "Fill shows close versus open. Color shows close versus previous close."
- **Line**: green if the last close on screen is at or above the first, red otherwise.
- **Area**: the space between the closing prices and the bottom is filled.
- **Baseline**: green above a horizontal baseline, red below; the baseline can be dragged on the y-axis.
- **Heikin-Ashi**: averaged candles that smooth price data.

Robinhood does not document volume, grid-line, session-break, crosshair, colour or time-zone switches.

## What Stockwatcher builds (chart-core `chartSettings.ts`, `chartTypes.ts`)

| Robinhood | Here | Notes |
|---|---|---|
| Chart types | Candles, Hollow candles, Bars (OHLC), Heikin-Ashi, Line, Area, Baseline | Colour rules above. Heikin-Ashi is computed over every loaded candle (its open is recursive, so starting at the first visible candle would redraw the candles while panning). The last-price tag, readout, tape, levels and orders keep real prices; the panel says so when Heikin-Ashi is chosen |
| Baseline reference | The previous close (the server's, else read off the candles); else the first visible close | Fixed, not draggable: a dragged baseline needs its own persisted per-symbol price and hit target, and the previous close is the reference a day trader reads |
| Y-axis scaling | Linear (price), Linear (%), Logarithmic, in the panel and in the price-axis menu | Percent is from the first visible candle's open (moves with the view), as before. Drawings, key levels, LULD bands, orders and the crosshair all map through the same scale (tested) |
| Auto-scale / double-tap | **Auto stretch** (default on): the axis fits what is drawn on screen and re-fits on pan, zoom, new data, a timeframe change and a ticker switch | Fits candles (closes for line types, HA values for Heikin-Ashi) plus overlay indicators, as TradingView's main pane does; key levels stay out. A price-axis drag makes it manual until the next ticker or timeframe, a double-click / double-tap on the price axis, or the small "A" shown while manual. Off: a new chart is fitted once, then held |
| "Adjust zoom settings" | **25 / 50 / 100 % of the trading day**, anchored at the latest candle | 100 % = the latest trading day's slots so far, pre-market to after-hours with extended hours shown, the regular session without; the thin overnight session is not counted. Floored at a quarter session so the first minutes are not two giant candles. Applied on ticker switch, timeframe switch and when chosen; wheel/pinch zoom still work after it |
| Extended hours | Shown (default, today's look) or hidden | Hidden drops pre-market, after-hours and overnight slots, so days abut with no gap. The premarket high/low level is still computed from all candles |
| Highlight pre-market / extended | "Highlight extended hours" (the existing session shading), on by default | |
| Display the high and low price | "High / low labels" on the highest and lowest price on screen | Real prices for every candle type |
| Visibility | Indicators, Drawings, Orders and positions | Hidden objects are not hit-tested |
| — | Volume on/off, Grid lines on/off, Session breaks (dashed line where each trading day starts) | Owner's list; standard TradingView settings Robinhood does not document |

Skipped on purpose:
- **Equivolume**: a box's width is its volume, which breaks the index-based time axis that gaps, drawings,
  indicators, the inspector and the zoom presets all rely on.
- **Scatter**: dots of the closes; the line shows the same points, and at the 0.25–2 px slot widths the presets
  produce, dots are unreadable.
- **Trend**: candles coloured by close vs previous close; hollow candles carry exactly that colour rule.
- **Fit all orders / positions**: they are simulated here and usually near price; auto stretch already
  includes indicators. Can be added if wanted.
- **Options price type**, **trading settings** (auto-send, time in force): no options and no real trading.
- **Auras**: an app-wide background theme, not a chart setting; the chart's session shading covers the
  per-candle part.

## Sources
- [Using advanced charts | Robinhood](https://robinhood.com/us/en/support/articles/using-advanced-charts/)
- [Using charts | Robinhood](https://robinhood.com/us/en/support/articles/using-charts/)
- [Widgets in Robinhood Legend | Robinhood](https://robinhood.com/us/en/support/articles/widgets-in-robinhood-legend/)
- [Auras on Legend | Robinhood](https://robinhood.com/us/en/support/articles/auras-on-legend/)
- [Robinhood on X, January 2025: "Customize charts your way…"](https://x.com/RobinhoodApp/status/1877719804003459378) (not readable without an account)
