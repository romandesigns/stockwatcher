# ross/pending agent status

- DONE: clients (0ec596cc + the server commit): shared model packages/shared/src/pending.ts, web PendingPanel (bottom row, first), mobile PendingPanel (first card), After hours tag. Screenshots C:\swt\pending-shots\. Contract: C:\swt\pending-contract.md (+ card field `session`).
- DONE: server crates/server/src/pending: board in the market core, statuses from ingest, news from deliver_evidence, tick on decide_rocket; ws pending.subscribe -> pending.snapshot / pending.update; welcome pending.v1; PENDING_CARD (default on) / PENDING_GRACE_SECS (120).
- DONE (T1 study): 19:50:00 ET corporate-action halts never make a card; no push change (T1 pushes stay the Halts alert's); no action wording; session carried for the After hours tag.
- DONE: 12 unit tests + tests/pending.rs ws test; fmt, clippy -D warnings, JS typecheck/lint/tests, web build.
- DONE: cargo test --workspace --no-fail-fast: 84 binaries, 1483 passed, 0 failed.
- KNOWN: headlines/urls null until the ross/news merge (pending::hit_from_item: two `None` lines -> item.headline.clone(), item.url.clone()).
