// The "No live quote" warning on an alert (owner, 2026-11-22; research v2 W2-REPORT §2 and
// 30-w2/L-x-quote.md "Coverage"): alerts that fired with no usable bid/ask in the minute before
// them reached +10 % from the ask before −5 % (good-v1) 11.5 % of the time, against 21.5 % for
// alerts with one. Without a quote there is also no spread chip (spread.ts), so the cost of
// trading it is unknown; this says so instead of showing nothing.
//
// Where it comes from: the server's spread stage (crates/server/src/spread.rs `Priced::NoQuote`)
// asks Alpaca for the quotes when the alert fires. Only an *answered* request with no market quote
// in it sets the flag: a failed or timed-out request is unknown and never shows the warning (it
// says nothing about the market). It arrives as `no_quote: true` on a Runners row and
// `noQuote: true` on a `runners.notice`. This module is the ONE place its words live, so the web,
// the desktop (the same web app) and the phone say the same thing.
//
// Colour: the warning status tone, never the accent (it is a caution, not an invitation), with
// the two-tone `quoteOff` mark, so it is never colour alone.

/** What a client draws for an alert with no live quote. */
export interface QuoteWarningView {
  /** "No live quote": the chip's text. */
  label: string;
  /** "No quote": the chip's text in a tight layout. */
  short: string;
  /** The screen reader's and the tooltip's sentence. */
  aria: string;
}

const VIEW: QuoteWarningView = Object.freeze({
  label: "No live quote",
  short: "No quote",
  aria: "No live quote when this alert fired, so its cost to trade is unknown. In research, alerts like this reached +10% before −5% about 1 in 8 times, against 1 in 5 with a quote.",
});

/** The warning for a wire flag, or null: only an explicit `true` shows it (absent, false, null: nothing). */
export function quoteWarningView(noQuote: unknown): QuoteWarningView | null {
  return noQuote === true ? VIEW : null;
}
