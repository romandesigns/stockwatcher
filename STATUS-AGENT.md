# ross/news agent status (2026-11-23 UTC)

Done (committed):
- 76e09069 clients: news source, Offering filed, No news confirmed, news-optional switch (web/mobile; desktop hosts web)
- 082ab645 server: pillars-v6 (alpaca, sec_edgar, press_wire, halt_t1), news seam (stock_context::SourceNews), SEC EDGAR collector (swimport evidence, OFF without EDGAR_USER_AGENT), T1 halts as news, ross-panel-v2 news-optional + synced switch detectors.ross-require-news
- 96b73364 clients: T1 "News pending (T1 halt HH:MM)" row flag, press wire names
- this WIP: tests for T1 and source-news loader, docs section 11

Audit 2026-11-20 (upper bound, local daily indexes):
- v5: 36 passed price+gain+RVOL, 0 passed news, 0 on panel
- v6 + EDGAR, news required: 9 passed news, 7 on panel (FTHAW GOVX MEDS SEGG TRUG UONE VWAV; IPFXU TDACU blocked by float unknown)
- v6 + EDGAR, news optional / v6 news optional without EDGAR: running

In progress / next:
- full check suite (cargo fmt/clippy -D warnings, cargo test --workspace --all-targets, JS typecheck/lint/tests, web build)

Known issues:
- alerts::tests::a_detector_switched_off_at_runtime_is_held_and_ledgered failed once under full-suite load, passes alone (timing; untouched code)
- T1: no halt price, no later T2/T3 codes (chart engine keeps one reason per halt span); replay seek board misses T1 before the seek point
- restart replays the day under the panel policy in force at restart
