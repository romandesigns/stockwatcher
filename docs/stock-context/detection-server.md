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

## Timing, persistence and restoration

Every record preserves its core input cutoff and semantic recognition time. The server adds
`emitted_at` only after releasing the market core lock. It is server emission telemetry, not
provider publication time or browser-render latency. `input_watermark` is the market's admitted
input sequence associated with that capture, not proof of complete feed acquisition.

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

No deployment or live provider probe is implied by local tests. Experimental detectors still
need a frozen comparison protocol and adequately covered forward cohorts before usefulness,
competitive latency or trading-performance claims.
