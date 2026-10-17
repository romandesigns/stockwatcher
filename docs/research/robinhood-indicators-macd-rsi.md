# Robinhood: MACD and RSI on mobile and web

Research note, 2026-10-17, written as a companion to `robinhood-chart-gestures.md`. It covers how Robinhood
handles the two oscillators we ship (RSI, MACD): how you add and edit them, how they are drawn, and how you
interact with them, on the app's advanced chart and on Robinhood Legend (web and desktop). It ends with how our chart compares.

**Confidence.** Robinhood's help center documents adding, editing, hiding and removing indicators, plus their
maths. It does **not** document how oscillator panes look or behave: pane size, legend values, resizing or
collapsing. One secondary source says Legend's charting engine "runs on TradingView's chart library". Rows
marked **TV** are TradingView's documented behaviour, which Legend would inherit if that is true. **Unverified**
marks things neither source confirms.

## 1. Adding, editing, removing

| What | Mobile (advanced chart) | Web / Legend | Source |
|---|---|---|---|
| Add | **+** → swipe the common ones, or **View all** → search → tap | Indicator icon at the widget's top right → pick | Using advanced charts; Chart indicators on Legend |
| Limit | Up to **10** | Not stated (80–100 available on Legend) | Using advanced charts; Technical indicators |
| Hide vs. remove | Tap an added indicator again to **hide** it; it stays in the list | Not stated; **TV**: eye icon in the pane legend | Using advanced charts |
| Edit | **Gear** on the indicator: settings, or remove | Settings per indicator (lengths, styling); the calculation can't be changed | Using advanced charts; Financial Tech Wiz |
| Sets | **Save as set**, reusable on any advanced chart and synced with Legend both ways | Folder icon → name the set; apply a set from the same menu | Using advanced charts; Chart indicators on Legend |
| Persistence | Preferences apply to the advanced chart of every stock | Same | Technical indicators |
| Show/hide all | ⋯ chart settings → show or hide indicators; visual markers for indicators | ⋮ settings | Using advanced charts |

## 2. The indicators themselves

| | RSI | MACD | Source |
|---|---|---|---|
| Maths | 100 − 100 / (1 + RS), **Wilder's smoothing** | Fast EMA − slow EMA, plotted against a signal line; histogram = MACD − signal | Technical indicators; Chart indicators on Legend |
| Defaults | Period not stated (**TV**: 14, close) | 12 / 26 / 9 | Chart indicators on Legend |
| Scale and levels | Fixed 0–100; **70 overbought, 30 oversold** | Around zero; read on crosses of the signal line | Same |
| Look | **TV**: one line, dashed 70/30 bands with the zone between **lightly shaded**, a faint 50 midline | **TV**: two lines plus a histogram in **four shades**: above or below zero, each growing or shrinking | Unverified for Robinhood |

## 3. Interaction

| Gesture or control | Mobile | Web / Legend | Source |
|---|---|---|---|
| Scrub | Long press scrubs; "more detailed data for a selected point in time". Whether oscillator values show is **not documented** | Hover crosshair; **TV**: each pane's legend shows its values at the crosshair | Using advanced charts |
| Zoom and pan | Pinch and swipe move every pane together | Wheel or trackpad zooms the x-axis | Using advanced charts; Widgets in Legend |
| Price axis | Drag to scale, double-tap to auto-scale | Right-click the y-axis → **Fit all indicators** | Using advanced charts; Widgets in Legend |
| Pane size | **Unverified** | **TV**: drag the border between panes | Unverified |
| Maximize a pane | **Unverified** | **TV**: double-click the pane | TV Charting Library shortcuts |
| Collapse a pane | **Unverified** | **TV**: the Collapse button (top right of the pane) or Ctrl/⌘ + double-click; again to restore | TV: How to collapse separate panes |
| Move a pane up or down; delete | **Unverified** | **TV**: pane buttons | TradeLocker (TradingView charts) |

## 4. Our chart against this

| Area | Robinhood / Legend | Ours (2026-10-17) | Gap |
|---|---|---|---|
| Add and remove | + menu, tap to hide, gear to remove | Indicator menu or sheet; tap toggles on or off | No **hide-but-keep**, which matters once settings are editable |
| Settings | Gear: lengths and styling | Fixed presets (RSI 14, MACD 12/26/9, …) | **No parameter editing** |
| Sets and sync | Named sets, synced web↔mobile | One global list per device, the same for every symbol | No named sets; no sync across devices |
| Count | 10 | 3 overlays + 2 panes | Deliberate: colour-blind safety |
| RSI look | 70/30 bands, shaded zone, 50 midline (TV) | Dashed 30/70 guides, shaded 30–70 zone, faint 50 line | **Done** 2026-10-17 |
| MACD look | Four-shade histogram (TV) | Four shades: above or below zero × growing or shrinking | **Done** 2026-10-17 |
| Pane legend | Values at the crosshair (TV) | Name, histogram and line values at the inspected candle, on a translucent backing | **Done** 2026-10-17 |
| Scrub | Long press scrubs | Long press scrubs; the inspection line crosses every pane and the legends follow it | Parity |
| Zoom and pan | Shared across panes | Shared | Parity |
| Pane size | Drag the border (TV) | Drag a pane's top edge (mouse or finger); heights persist; panes together never exceed 60 % of the chart | **Done** 2026-10-17 |
| Maximize / collapse | Double-click / button (TV) | Double-tap or double-click a pane: expand to 40 %, again to restore | Expand **done**; collapse not built |
| Y-axis | "Fit all indicators" | Oscillator panes always auto-fit; RSI is fixed 0–100 | Parity in effect |

## 5. Recommended next steps (smallest first)

1. **Look.** Shade RSI's 30–70 zone and add a faint 50 line. Draw MACD's histogram in four shades. Add the
   histogram's value to the MACD legend. Give pane legends a translucent backing so lines don't cross the text.
   Shared chart-core layout, so web and phone get it together.
2. **Pane size.** Phone: drag the gap above a pane to resize it, and double-tap a pane to maximize or restore
   it. Web: drag the border; double-click to maximize. Heights persist.
3. **Settings.** A gear per indicator: RSI period and levels; MACD fast, slow and signal. Tap to hide while
   keeping the settings; a separate remove.
4. **Later.** Named sets, and sync across devices through the server.

## Sources
- [Using advanced charts | Robinhood](https://robinhood.com/us/en/support/articles/using-advanced-charts)
- [Technical indicators | Robinhood](https://robinhood.com/us/en/support/articles/viewing-indicators/)
- [Chart indicators on Legend | Robinhood](https://robinhood.com/us/en/support/articles/chart-indicators-on-legend/)
- [Widgets in Robinhood Legend | Robinhood](https://robinhood.com/us/en/support/articles/widgets-in-robinhood-legend/)
- [Robinhood vs TradingView | Financial Tech Wiz](https://www.financialtechwiz.com/post/robinhood-vs-tradingview/) (secondary: "Legend's charting engine runs on TradingView's chart library")
- [How to collapse separate panes? | TradingView](https://www.tradingview.com/support/solutions/43000665813-how-to-collapse-separate-panes/)
- [Keyboard shortcuts | TradingView Charting Library](https://www.tradingview.com/charting-library-docs/v29/getting_started/Shortcuts)
- [How to add, remove, collapse or move TradingView indicators | TradeLocker](https://tradelocker.com/docs/how-to-add-remove-collapse-or-move-tradingview-indicators/)
