# StockWatcher Issue #63: bounded local bridge pilot

This is a **synthetic-only, supervised pilot**, not a general task runner. It invokes the actual installed Codex CLI to implement an inert arithmetic recipe, a fresh Claude Code CLI session to independently review that exact artifact, and another fresh Claude session to execute an approved fixed MCP fixture. The tool exposes only six built-in arithmetic cases. Model-generated shell commands and executable code are never evaluated.

## Roles and workflow

Codex: implementation. Claude: independent review, then separately gated execution. Coordinator: validation, provenance, cancellation and handoffs. The connected GitHub account records each event on romandesigns/stockwatcher Issue #63. No user prompt copying is needed.

States: queued, codex-working, claude-review, approved, claude-executing, accepted. A rejected review terminates at changes-requested. Any process, auth, validation, path, source, timeout or relay error stops progression. There are no automatic retries or automatic stale-lock reclamation. One global exclusive lock means at most one agent process at a time.

Every event records task/run ID, sequence, actor, time in UTC, decision, exact source commit, base commit, artifact hashes and previous-event hash. Before advancing, the trusted host relay must post the exact event to Issue #63 and write an acknowledgement with its SHA-256 and the returned GitHub comment ID. A missing/mismatched receipt blocks execution. The pilot does not parse arbitrary issue text as commands or treat issue comments as owner approval.

## Local commands

Use Node 24. No package installation is needed. From this directory:

- node --test core.test.cjs fixture-server.test.cjs
- node bridge.cjs init
- node bridge.cjs step
- node bridge.cjs status
- node bridge.cjs stop

The operator creates .runtime/config.json before init with repository, allowed_checkout and binaries (codex, claude, git, node), each containing an absolute path and sha256. The current checkout must be the isolated branch codex/issue63-bridge-dry-run with the expected origin. Never use the H: checkout. Code and binary hashes are pinned at initialization. No PATH-based agent resolution occurs during execution.

A relay acknowledgement .runtime/ack-N.json has repository romandesigns/stockwatcher, issue_number 63, comment_id returned by the GitHub connector and event_sha256 for the exact bytes of .runtime/event-N.json. The relay is trusted host code, not an agent tool. This is an evidence link, not a cryptographic GitHub signature. Unattended GitHub consumption needs a separately verified authenticated transport and is not implemented by this pilot.

Create .runtime/STOP or run the stop command to deny new work and stop the current invocation. The parent monitors STOP every 400 ms and kills only the child process tree it started. Agent timeout: 240 seconds; output caps: 500 KB stdout, 100 KB stderr; Claude per-invocation budget cap: USD 1 (existing subscription auth, no new credentials or subscriptions). No automatic cost top-ups. A crash leaves a lock for inspection instead of silently launching a duplicate. State/event writes are crash-conservative; no automatic repair or resume after ambiguous execution.

## Threat model and authority

Untrusted inputs include agent output, external issue prose and tool output. Only the fixed task ID and a tiny declarative operation allowlist are accepted. Unknown fields, arbitrary paths/code, wrong hashes, rejected reviews, bad outputs and missing execution evidence fail closed. Execution checks both the review and its scope-limited approval. Coordinator independently verifies all six outputs; it never accepts a model's unsupported success claim. Review and execution use new sessions without inherited conversation history.

Claude has no built-in tools; reviewer has no MCP; executor gets only the fixed arithmetic MCP tool. Customizations/hooks are disabled and no broker, cloud storage, deployment or production MCP is configured. Codex uses read-only sandboxing, no approvals, disabled integrations/hooks/shell/plugins and a tool-free output contract. Existing credentials remain in their existing stores; only selected environment keys are inherited. Logs redact common token patterns, but are private and ignored by Git; never publish raw logs without inspecting them.

This is not isolation against a malicious Windows process running as the same user. CLIs can access their own authentication stores and contact their model providers. Filesystem ACL isolation, a general worktree executor, task ingestion, signed human approval, scheduling, automatic remediation/retry and 24/7 reliability remain future work. No production, deployment, protected branch merge, restore/migration, market data, trading or credential-management operation is implemented or approved. Expanding capabilities requires a separate reviewed design and explicit authorization where required by Issue #63.

## Discovery (2026-11-11)

- Codex CLI 0.167.0-alpha.3.2, existing ChatGPT auth.
- Claude Code 2.1.322, existing Claude subscription auth. Another 2.1.307 executable exists; the pilot pins the verified 2.1.322 binary.
- Git 2.55.0.windows.5; Git transport read access verified.
- Local H:/wavystack/stockwatcher main ba8751e9331101f80617dd5adbea6586bd289e8a is 91 commits behind remote main 3b85ae9ee190edb2bae45860fa7219fe46bf6ae3. Limited tracked status was clean; untracked content was not audited.
- P1 0e851cdbe16a7f5bca1e1815976418a21a6b26f1 and X1-protocol 38862013dc0e686463289532f0e1b6fe580c6b54 were present in local worktree metadata; their queried branch names were absent remotely. They remain untouched.
- The original H: Git directory also backs many C: worktrees. A new GitHub clone on C: was used so this pilot does not mutate those shared metadata.
- gh is installed but not authenticated. Connected GitHub relay is used for this supervised pilot. No polling daemon, scheduled task or persistent background worker is installed.
- Main pushes trigger release/deployment. This pilot never pushes main or opens a CI-triggering PR.
