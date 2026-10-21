# Detection Panel server contract

The Detection Panel is experimental discovery. `DETECTION_PANEL=1` opts in; the default is off.
`STOCK_CONTEXT` controls Ross admission/publication independently. The shared stock-context
Engine owns detector mathematics, levels and transitions. The server does not implement another
detector, derive order flow from prints, or enable push/audio/order execution.

Each Market owns its own Engine and transport runtime. The dashboard uses `live`, practice uses
`replay`; a synthetic provider is persistently labeled `fixture`. Chart subscriptions and chart
display timeframes do not drive detector monitoring. Replay resets start a new transport epoch.
Core semantic IDs remain deterministic; a delivery belongs to `(runEpoch, transition_id)`.

## Socket and HTTP

After the existing authenticated `hello`/`welcome`, send `detection.subscribe` to receive a
`detection.snapshot`. `detection.unsubscribe` removes the opt-in. Changed state is a
`detection.update`, at most four times per second. Every frame is a bounded full replacement,
not a partial delta:

```text
{ type, enabled, source, runEpoch, cursor, inputWatermark, snapshot,
  transitions: [{ sequence, emitted_at, input_watermark, restored, transition }],
  history: { capacity, dropped, first_sequence, last_sequence },
  telemetry: { engine, journal_queue_dropped, journal_written, journal_write_errors,
               durable_target, tail },
  provenance: { schema, definition, source, boot, code_commit, code_identity },
  replacement: true, experimental: true, alertsEnabled: false }
```

`snapshot` and each `transition` use stock-context's authoritative snake_case structs.
Envelope `cursor` orders replacement frames within an epoch; individual transition `sequence`
orders significant history. Several ordinary state changes can coalesce into one replacement
without losing a significant transition. A client applies only a newer cursor for its current
epoch, deduplicates transitions by stable ID, and requests a fresh subscription on an unexplained
transition-history gap. An explicit new-epoch snapshot replaces the old run. Old-epoch broadcasts
are discarded at the server. The existing broadcast `Lagged` and replay `Resync` paths resend a
snapshot. Reconnection returns state/history and never replays live notifications.

`GET /api/detection?source=live|replay` and `/api/detection/export?source=live|replay` expose the same
bounded snapshot through the existing bearer-protected routing. Export is a research seam,
not an exhaustive corpus: inspect retained-history, engine and journal loss counters. A new
client or research tool must select its market explicitly.

`profile` reports the actual active band as `price_min_micros` / `price_max_micros`, plus the
policy's `research_min_micros` / `research_max_micros`. `policyHash` is its full byte hash.
Optional `context[symbol]` carries authoritative `ross_member` (including retained members;
null when Ross is disabled) and news-policy `catalyst: qualifying|none|unknown`. These badges
are independent of detection. `rebuilding` marks partial seek state.

## Timing, persistence and restoration

Every record preserves its core input cutoff and semantic recognition time. The server adds
`emitted_at` only after releasing the market core lock. It is server emission telemetry, not
provider publication time or browser-render latency. `input_watermark` is the market's admitted
input sequence associated with that capture, not proof of complete feed acquisition.
The inherited engine boundary is strict `admit < bar_end + settle`, clock first at equality.
Detection preserves this documented legacy exception to the handoff's inclusive prose.
Transition drain never walks the universe; full snapshot/context copies follow the 250ms
display tick. `snapshot_captures`, `capture_batches` and `transitions_captured` expose this work.

Significant transitions are retained in a 2,048-record immutable ring and offered to a separate
bounded writer. Retention eviction, journal queue saturation and write failures are counted.
The append happens on a dedicated thread, never under the core lock. An unavailable durable
target is explicit and does not block state delivery. A successful append/flush is not a
power-loss durability guarantee or a whole-market completeness claim.

Logs use a create-new per-source/boot segment, with schema/definition and executable/commit
identity in the header. An offline reader is byte/line/record/epoch bounded and rejects unknown
headers, sequence gaps/disorder and torn records. `prefix_integrity` concerns the readable prefix;
`tail: unverified` remains explicit at EOF. There is no whole-epoch completion seal. Startup logs
are research history and are never fed back into the live alert path. Replayed/rebuilt history
is labeled restored; seek/current state are recoverable without a stale-alert burst.
All attempted journal records (including documentary data-status changes with zero active
episodes) receive a journal sequence before queue admission. The reader detects interior
status loss as well as transition loss; a missing final record remains an unverified tail.
The header's detector policy comes from the actual first configured engine; each epoch's status
records also carry its actual snapshot policy. They do not independently guess a default profile.

A recoverable provider error opens a documentary gap and interrupts existing setups. For sources
without link messages, the next received normalized event closes that error gap and requires
post-gap qualification; it never restores old anchors or claims continuous coverage. An explicit
link-down remains interrupted until link-up, including when buffered prints arrive. An unavailable
source starts its interruption at the configured replay day/start (ET 04:00 fallback), or actual
local admission time for live, never epoch zero.

## Local references and runnable fixture

`DETECTION_QUIET_REFERENCES=/local/references.jsonl` optionally provisions raw Rust
`QuietReference` records at startup: at most 16MiB, 1MiB per line and 16,384 lifetime identities.
No file is fetched. Identity/version conflicts are refused; accepted versions remain available
across a replay seek. The core validates date, prior-through, segment, basis and observed time
at each join. Synthetic references are refused outside an explicitly labeled fixture market.
Absent/invalid references leave quietness unavailable while price-only families continue.

```powershell
cargo run -p stockwatcher-server --example detection_fixture -- micro_valid
cargo run -p stockwatcher-server --example detection_fixture -- micro_valid --serve --static apps/web/dist
```

The first prints an actual Market snapshot. The second binds only `127.0.0.1:8792`, exposes
`/api/detection?source=replay` and the ordinary socket/UI routes, and keeps live idle. Choose
the replay source explicitly. Inputs come from the core owner's adversarial scripts, pass
through real Market/chart verdict/revision/clock handling and are `fixture` / `SyntheticFixture`.
The fixture seam rejects live markets and nonfixture event sources. Its declared complete
synthetic input is not evidence of real feed completeness. It never starts a live provider.

No deployment or live provider probe is implied by local tests. Experimental detectors still
need a frozen comparison protocol and adequately covered forward cohorts before usefulness,
competitive latency or trading-performance claims.
