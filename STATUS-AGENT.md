# ross/news agent status (2026-11-23 UTC)

Done (committed):
- 76e09069 clients: news source, Offering filed, No news confirmed, news-optional switch (web/mobile; desktop hosts web)
- 082ab645 server: pillars-v6 (alpaca, sec_edgar, press_wire, halt_t1), news seam (stock_context::SourceNews), SEC EDGAR collector (swimport evidence, OFF without EDGAR_USER_AGENT), T1 halts as news, ross-panel-v2 news-optional + synced switch detectors.ross-require-news
- 96b73364 clients: T1 "News pending (T1 halt HH:MM)" row flag, press wire names
- this WIP: tests for T1 and source-news loader, docs section 11

Audit 2026-11-20 (upper bound, local daily indexes):
- v5: 36 passed price+gain+RVOL, 0 passed news, 0 on panel
- v6 + EDGAR, news required: 9 passed news, 7 on panel (FTHAW GOVX MEDS SEGG TRUG UONE VWAV; IPFXU TDACU blocked by float unknown)
- v6 + EDGAR, news optional (ross-panel-v2): 26 on panel (news no longer counted; 9 still pass news)
- v6 news optional without EDGAR: 26 on panel, 0 pass news

Checks (all green): cargo fmt --check, clippy --workspace --all-targets -D warnings, cargo test --workspace --all-targets (92 test binaries, 0 failures; no doctests locally), JS typecheck 8/8, lint, tests (shared 338, web 101, mobile 122, app-state 71, desktop 6, chart-core 564, chart-web 17), web build.
- T1 halts at exactly 19:50:00 ET (scheduled reverse-split halts) excluded, with a test.

Known issues:
- alerts::tests::a_detector_switched_off_at_runtime_is_held_and_ledgered failed once under full-suite load, passes alone (timing; untouched code)
- T1: no halt price, no later T2/T3 codes (chart engine keeps one reason per halt span); replay seek board misses T1 before the seek point
- restart replays the day under the panel policy in force at restart
