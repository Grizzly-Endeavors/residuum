# Feedback Submission — Residuum Client Plan

What needs to change in `residuum` to ship the feedback / bug-report submission feature. See `~/Projects/residuum-feedback-decisions.md` for the full architectural context.

Nothing about server-side storage is covered here.

---

## Cross-repo contracts you MUST honor

Before picking an implementation shape, know this:

**Wire contract is `project-residuum/feedback-ingest/src/types.rs`.** The ingest service uses `#[serde(deny_unknown_fields)]` on both request structs — any extra field the client sends will be rejected with 422. The canonical wire format for the two endpoints is:

```jsonc
// POST agent-residuum.com/api/v1/bug-report
{
  "kind": "bug",                            // REQUIRED top-level tag
  "what_happened": "...",
  "what_expected": "...",
  "what_doing": "...",
  "severity": "broken" | "wrong" | "annoying",  // enum — any other value = 422
  "client": {
    "version": "...",                        // required
    "commit": "..." | null,                  // optional
    "os": "...",                             // required
    "arch": "...",                           // required
    "model_provider": "..." | null,          // optional
    "model_name": "..." | null,              // optional
    "active_subagents": [ { "name": "...", "status": "..." } ],   // required (may be empty)
    "config_flags": { "key": "value" }       // required (may be empty object)
  },
  "spans_otlp_b64": "<base64 of OTLP protobuf>"   // REQUIRED top-level
}
```

```jsonc
// POST agent-residuum.com/api/v1/feedback
{
  "kind": "feedback",                        // REQUIRED top-level tag
  "message": "...",
  "category": "..." | null,                  // optional
  "client": { "version": "..." }             // feedback client context is version-only
}
```

The Rust `BugReport` / `Feedback` types described further down in this plan may exist as internal ergonomics — what matters is that whatever `submit()` serializes MUST match the JSON above byte-for-byte (after serde).

**Where the request goes.** The client POSTs to `agent-residuum.com/api/v1/{bug-report|feedback}` — the **relay**, not the ingest service directly. The relay attaches the `X-Feedback-Upstream-Token` header on the way through. **The client does NOT know or care about that header** and must not send it.

**Response shape (both endpoints):**

```jsonc
{ "public_id": "RR-XXXXXXXXXX", "submitted_at": "2026-04-15T14:23:00Z" }
```

4xx/5xx responses return `{ "error": "<terse>" }`. The relay passes status + body through unchanged.

**Severity lock.** Values are exactly `broken`, `wrong`, `annoying`. The old `crash`/`wrong_output`/`confusing`/`slow` values are obsolete and not accepted by the DB's CHECK constraint. If you find references to them in any doc still, those docs are stale.

---

## Tracing service

`src/tracing_service/mod.rs`

Replace `send_bug_report(message: &str)` with two typed methods:

```rust
pub async fn send_bug_report(report: BugReport) -> Result<SubmissionReceipt>
pub async fn send_feedback(feedback: Feedback) -> Result<SubmissionReceipt>
```

Both delegate to a single private `submit()` helper that handles serialization, the HTTP POST to `agent-residuum.com/api/v1/{bug-report|feedback}`, and response decoding. The reqwest client is the shared one already used by `UserEndpoints` export, with a 30s timeout.

**Sanitization:** force `sanitize_content = true` for the span dump regardless of the runtime setting. This is applied before serializing the OTLP payload, not after.

**Span trimming:** trim the OTLP batch to stay under ~4 MB before serialization. If spans are dropped, log a `warn` with the count. Do not fail the submission.

**Stubs to leave alone:** `on_error()` and `auto_error_reporting` stay exactly as they are. This feature does not touch the auto-reporting path.

**Submission endpoint:** configurable via `config.toml` under `[tracing]`, e.g.:

```toml
[tracing]
feedback_endpoint = "https://agent-residuum.com"  # default
```

This lets local dev point at a mock without recompiling.

### New types

```rust
pub struct BugReport {
    pub what_happened: String,
    pub what_expected: String,
    pub what_doing: String,
    pub severity: Severity,
    pub client: ClientContext,
}

pub struct Feedback {
    pub message: String,
    pub category: Option<String>,
    pub client: ClientContext,
}

pub enum Severity {
    Broken,
    Wrong,
    Annoying,
}

pub struct SubmissionReceipt {
    pub public_id: String,    // e.g. "RR-01HZKX2P7M"
    pub submitted_at: String, // ISO-8601
}
```

---

## Client context gatherer

New module: `src/tracing_service/client_context.rs`

Two entry points:

```rust
pub async fn gather_for_bug_report(config: &Config, ...) -> ClientContext
pub fn gather_for_feedback(config: &Config) -> ClientContext
```

**`gather_for_bug_report` collects:**
- `version`: `env!("CARGO_PKG_VERSION")`
- `commit`: injected at build time via `build.rs` (add `vergen` or equivalent if not already present; check if the build already captures git metadata)
- `os`, `arch`: `std::env::consts::{OS, ARCH}`
- `model_provider`, `model_name`: active LLM config from the resolved config
- `active_subagents`: list of `{name, status}` from the subagent registry at submission time
- `config_flags`: a pre-curated allowlist of boolean toggles and short enum values from the resolved config — no secrets, no file paths, no API keys (exact key list TBD during the config resolver audit)

**`gather_for_feedback` collects:**
- `version` only

Explicitly excluded from both: chat history, memory contents, file contents, API keys, file paths.

---

## CLI: `residuum bug-report`

`src/commands/bug_report.rs` — expand from the current single-flag form.

**Modes:**

| Mode | Trigger | Behavior |
|---|---|---|
| Flags | All four required flags provided | Validate and submit immediately |
| Editor | No required flags | Open `$EDITOR` with a templated markdown form; parse on save |
| Pipe | stdin is not a tty | Read markdown from stdin and parse |

**Flags:**
- `--happened <TEXT>` (required in flags mode)
- `--expected <TEXT>` (required in flags mode)
- `--doing <TEXT>` (required in flags mode)
- `--severity <broken|wrong|annoying>` (required in flags mode)
- `-m/--message` — kept for backward compatibility; if provided alone maps to `--happened`

**Editor template** (`$EDITOR` is opened with a temp file pre-populated):

```markdown
## What happened?
<!-- Required: describe the actual behavior -->

## What did you expect?
<!-- Required: describe what should have happened -->

## What were you doing?
<!-- Required: steps or context -->

## Severity
<!-- Required: one of: broken, wrong, annoying -->
```

Parsing uses a simple `## Heading` splitter; no markdown parser dependency.

**On success:** print the `public_id` and a one-line hint:

```
Bug report submitted: RR-01HZKX2P7M
Reference this ID in a GitHub issue if you have more to add.
```

---

## CLI: `residuum feedback`

New file: `src/commands/feedback.rs`

```
residuum feedback -m "message text" [-c category]
```

Single-shot, no editor mode. Intentionally low-friction.

Register in `src/commands/mod.rs` and wire into the top-level clap enum.

**On success:**

```
Feedback submitted: RR-01HZKX2P7M
```

---

## Agent tools

Two new built-in tools registered in the tool set:

**`file_bug_report`**
- Required params: `what_happened`, `what_expected`, `what_doing`, `severity`
- Calls `TracingService::send_bug_report()`
- Returns `public_id` in the tool result

**`submit_feedback`**
- Required params: `message`
- Optional params: `category`
- Calls `TracingService::send_feedback()`
- Returns `public_id` in the tool result

Tool descriptions should clearly distinguish the two surfaces: `file_bug_report` is for broken behavior, `submit_feedback` is for confusion, usability problems, or the agent noticing patterns in its own behavior that seem off. The latter is an explicit goal — agents should use it.

---

## Web UI

`residuum/web/` (Svelte 5 SPA)

- Persistent footer link labeled **Feedback** that opens a modal
- Modal has a tab toggle at the top: `Report a bug` | `Send feedback`
- Each tab renders its own form with client-side validation for required fields
- Forms POST to the residuum gateway (not directly to the relay); the gateway handles the `TracingService` call

**Gateway endpoints:**

- `POST /api/tracing/bug-report` — already exists in `src/gateway/web/tracing_api.rs`; expand `BugReportRequest` from `{ message }` to the full structured body
- `POST /api/tracing/feedback` — new handler in `tracing_api.rs`

**Success state:** show the `public_id` with a copy button and the same GitHub issue hint as the CLI. Keep the modal open on success so the user sees the ID before closing.

---

## Still open

- **Config flag allowlist:** which keys from the resolved config are safe to auto-attach. Requires a pass through `ConfigResolver` — probably done as part of implementing the gatherer, not before.
- **Editor mode template:** whether to include a read-only preview of the auto-attached context (version, active subagents, etc.) so the user can see what's being sent. Useful for transparency; adds implementation complexity.
- **Empty buffer warning:** whether to warn the user if the span buffer is empty at submission time (indicates the tracing layer may not be running), since the bug report would still be useful but the trace context would be missing.
- **`vergen` or equivalent:** check if `build.rs` already captures git commit metadata before adding a new build dep.
