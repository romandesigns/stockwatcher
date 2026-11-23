# ross/wires agent status (2026-11-23 ~02:55 UTC)

## Done
- Source check written up at `C:\wavystack\outputs\stockwatcher-ops\WIRES-SOURCES.md`. **No wire approved.**
  - PR Newswire: the feed works (20 items per fetch, RFC 2822 `+0000`, ticker only in the teaser text), but the Terms of Use forbid "any robot, spider, site search/retrieval application, or other manual or automatic device or process to access, search, download, retrieve, index or "data mine" any content or data from the Site". Excluded.
  - GlobeNewswire: Akamai 403 to an identified client on every feed and on the terms page. Excluded.
  - Business Wire: all-news feed "deactivated by the administrator", the earnings feed has 0 items, the site returns 403. Excluded.
  - Accesswire: robots.txt has `Disallow: /rss` and `/rss2`. Excluded.
  - Newsfile: AWS WAF challenge (202, empty body) on every path. Excluded.
- `crates/server/src/wires/` (parse, tickers, classify, collector, poller, archive) with 14 tests and fixtures.
  - `WIRES_NEWS` is off by default and there is no built-in feed URL (`WIRES_FEEDS=id=url`).
- Checks: fmt, workspace clippy `-D warnings` and `cargo test -p stockwatcher-server` are green, including `--test market` (47 tests). Only the doctest fails, because rustdoc is missing on this machine.

## Next
- At merge with ross/news: replace `LogSink` with a `SourceNewsSink` that writes `SourceNewsRecord`s (see the hand-back).

## Known issues
- PRN's 20 items per fetch would drop releases during weekday bursts.
- The client label for `press_wire` (`packages/shared/src/ross.ts rossNewsSource`) is not done here.
