# StockWatcher durable coordinator (Issue #64)

A local, default-disabled, supervised read-only coordinator. Codex produces an inert plan; a fresh Claude session reviews its exact hash; a separate Claude session calls only the approved inventory MCP tool. GitHub records hash-chained transitions through the owner's explicitly authorized gh login. No manual copying of prompts or results is required.

Only preservation_inventory is supported. High-risk kinds (production, live trading, real-data restoration, migrations, deletion, deployments, protected-branch writes, credential management and paid services) are rejected regardless of approval text. There is no generic command executor. New task kinds require implementation and independent review.

## Location and setup

Source is scripts/agent-coordinator on branch codex/issue64-durable-coordinator. The original scripts/agent-bridge and its original #63 checkout remain unchanged. Node 24 built-in SQLite is used; no dependency installation.

Runtime config, database, private agent logs, detached task worktrees and STOP live in the ignored .state directory in this isolated C: checkout. config.json contains repository, issue, branch, checkout, userProfile, ghConfigDir, and binaries. Each binary entry (node, git, gh, powershell, codex, claude) has an absolute path and SHA-256. It contains no token. GH_CONFIG_DIR must point at the existing Windows GitHub CLI config, because this execution host omits normal profile variables. Credentials stay in the existing Windows keyring; token scopes remain broader than the program's API allowlist.

## Lifecycle

Run with the configured Node executable:

- node control.cjs init — initialize disabled, STOP set. Do not use to reset a live worker.
- node control.cjs enqueue <authorization.json> — validate a fixed task and its owner grant, confirm GitHub identity and Issue #64, persist once. Duplicate ID with different content fails.
- node control.cjs start --supervised — launch a hidden local worker for already approved queued work; it stops when the queue is terminal.
- node control.cjs start --supervised --until-idle — run approved queued work and disable when done.
- node control.cjs stop — set STOP and disable. Running children notice within 500 ms; only this invocation's child tree is terminated.
- node control.cjs restart --supervised [--until-idle] — stop, wait for lease release, validate again and restart. Ambiguous prior execution is blocked rather than repeated.
- node control.cjs status — inspect tasks, last error, lease and receipt counts.

No startup service or scheduled task is installed. A machine reboot does not launch anything. An expired lease is reclaimed only if the prior PID is no longer alive; a reused PID can require manual inspection. No lock is deleted merely because time elapsed. A valid completed receipt recovers an interrupted execution; missing evidence blocks it. The worker disables itself and sets STOP on error or normal bounded completion.

Authorization JSON contains spec and grant. spec: id, repository, issue, kind, source_sha. grant: task_sha256, scope preservation-read-only, expires_at (at most 24 hours), authorized_by romandesigns, authorization_source owner-chat-2026-11-11-issue64-resume. The host/operator creates this only from the actual owner instruction, and the journal hashes it. This is a trusted-host attestation, not a user digital signature. Agent output or agent-authored GitHub comments cannot create or expand it. The initial grant covers the single Stage C read-only task only.

## Exact allowlists

GitHub: authenticated user metadata; romandesigns/stockwatcher metadata; Issue 64 and its comments; the exact coordinator branch ref. The only mutation is POSTing an audit comment to Issue 64. Every receipt is fetched back and checked for exact body, issue URL, account and event hash. An ambiguous POST is reconciled by marker. A persisted POST intent prevents resending if the receipt is not yet visible; duplicate/edited/deleted receipts stop progression.

Local commands: pinned git (read-only metadata, and creating an isolated detached worktree in .state only); pinned agent CLIs with fixed arguments; pinned Node MCP server; pinned PowerShell with one literal Get-ScheduledTask/Get-ScheduledTaskInfo query for sw-borrow-recorder-temp. taskkill is limited to the child PID launched by this invocation. No shell command, file path, executable or URL comes from issue prose or model output.

Inventory paths are the five literal report files in inventory.cjs plus stat-only metadata for dated G:/stockwatcher-durable-backup tar files. No archive payloads are opened. J:/I:, frozen X1, production services, Pi originals and market datasets are not writable or executable through the tool. Existing report text is data. The output separates current observations from dated assertions and explicitly marks Pi/VPS live state unverified where remote access was unavailable.

## Validation and limits

Run node --test coordinator.test.cjs crash.test.cjs host.test.cjs. Tests use disposable OS temporary directories and fake agents/GitHub; crash tests really exit a separate process without closing its SQLite connection. The original Phase 1 arithmetic pilot is not rerun.

One worker, one agent process at a time; two maximum remediation retries; 180 seconds per agent; 20 minutes per task; output and queue limits; free-space floor; default disabled. Details and threat model: ARCHITECTURE.md. Same-user malicious processes, OS-enforced memory limits, unattended host startup, general repository editing, and broad task discovery are not claimed. This implementation supports supervised read-only readiness only, conditional on tests, independent source review and the actual Stage C run.

## Operator reconciliation

Enqueue and supervised start must be adjacent: the 20-minute deadline starts at enqueue. Expired tasks require a new separately authorized task ID, never an automatic grant renewal. After an ambiguous GitHub POST, keep STOP set. Inspect Issue 64 for the exact run/sequence/hash marker; restore access and allow receipt reconciliation if found. If still absent, leave the original event and intent intact, record the unresolved outcome and operator decision on Issue 64, and use a new reviewed task/run only after explicit operator authorization. This version deliberately provides no automatic intent-clearing or force-replay command.
