# Residuum Code Review Audit — 2026-03-17

Conducted by a three-agent review team (`arch-reviewer`, `error-reviewer`, `api-auditor`) against CLAUDE.md standards.

---

## Architecture — 80% compliant, 3 violations

**Root cause of all violations:** three `mod.rs` files contain logic instead of declarations/re-exports only (CLAUDE.md requires mod.rs for declarations and re-exports only).

| Priority | File | Lines | Fix |
|---|---|---|---|
| Critical | `src/inbox/mod.rs` | 579 | Split into `types.rs` + `store.rs` |
| High | `src/models/mod.rs` | 623 | Move types to `types.rs`, mod.rs becomes ~30 lines |
| Medium | `src/tools/mod.rs` | 304 | Move `ToolError`/`ToolResult`/`ToolFilter` to `types.rs` |
| Minor | `src/memory/mod.rs` | 2 fns | Extract `strip_code_fences`, `parse_minute_timestamp` to `utils.rs` |

**Strengths:**
- No circular dependencies detected
- No god files — all large files (500+ lines) have single, clear responsibility
- No type scattering — related types are grouped correctly, just in wrong files
- Excellent module naming across all 20 top-level modules
- Max nesting depth of 3 levels — appropriate
- Gateway hub pattern (imports from 12+ modules) is correct for system assembly, not a violation

**Estimated fix effort:** 2–4 hours.

---

## Error Handling — Strong fundamentals, 11 silent failures

**No `unwrap`/`panic` in production code** — all properly gated behind test modules with `#[expect(clippy::unwrap_used)]`. Context chains, structured logging, and user-facing messages are well-done.

### Critical: Silent `.ok()` drops in gateway control flow

These drop errors with no log entry, violating the "no silent failures" rule. Control-flow signals (reload, shutdown, adapter restarts) can fail invisibly.

**High-risk (signal/control flow):**

| File | Line | Issue |
|---|---|---|
| `src/gateway/ws.rs` | 171 | Reload signal send silently fails |
| `src/gateway/ws.rs` | 183 | Server command send silently fails |
| `src/gateway/reload.rs` | 361 | Gateway shutdown signal lost |
| `src/gateway/reload.rs` | 396 | Graceful shutdown wait silently fails |
| `src/gateway/reload.rs` | 463, 506, 541 | Adapter shutdown signals (Discord, Tunnel, Telegram) silently fail |
| `src/gateway/reload.rs` | 520 | Tunnel status update silently fails |
| `src/gateway/setup.rs` | 79 | Setup server graceful shutdown signal lost |
| `src/gateway/event_loop/http.rs` | 130 | HTTP server graceful shutdown silently fails |
| `src/gateway/web/update.rs` | 75 | Event loop restart signal lost |
| `src/gateway/web/cloud.rs` | 166 | Cloud reload signal lost |

**Fix pattern:**
```rust
if let Err(e) = sender.send(...) {
    warn!(error = %e, "failed to send {context}");
}
```

**Medium-risk (file I/O):**

| File | Lines | Issue |
|---|---|---|
| `src/gateway/watcher.rs` | 19, 29 | File mtime checks silently treat unreadable files as unchanged |

**Low-risk (acceptable):**
- `src/config/wizard.rs:88,302,343` — stderr flush in setup wizard, best-effort UI
- `src/tunnel/connection.rs:73,77,132,226` — tunnel status broadcasts on a closing channel

### Additional log quality notes
- `src/gateway/ws.rs:76` — `warn!("failed to serialize server message")` missing error field; should be `warn!(error = %e, "...")`

**Strengths:** Structured logging discipline strong across 35+ sampled sites, appropriate log levels, no log spam, plain-language user-facing errors.

---

## API Visibility — 4/5, no blocking issues

Strong private-first discipline throughout:
- 30 `pub(crate) mod` declarations correctly hide internal subsystems
- 184 `pub(crate)` items appropriately distribute internal visibility
- Models layer exposes only traits + DTOs, hides provider implementations
- No internal test helpers accidentally leaked
- No large structs with all-public fields that should be encapsulated

**Minor finding:**
- `tools::line_hash()` is `pub` but only used internally by `edit.rs` and `read.rs` within the tools module — could be `pub(super)`, low priority

**Intentional (not violations):**
- macOS notification helpers (`parse_category`, `parse_priority`, etc.) are public for legitimate cross-module config parsing use

---

## Action Items by Priority

| # | Issue | File(s) | Effort |
|---|---|---|---|
| 1 | Add `warn!` to 11 silent `.ok()` drops | `gateway/ws.rs`, `gateway/reload.rs`, `gateway/setup.rs`, `gateway/event_loop/http.rs`, `gateway/web/*.rs` | Small |
| 2 | Refactor `inbox/mod.rs` | `src/inbox/mod.rs` | Medium |
| 3 | Refactor `models/mod.rs` | `src/models/mod.rs` | Medium |
| 4 | Refactor `tools/mod.rs` | `src/tools/mod.rs` | Small |
| 5 | Extract memory utils | `src/memory/mod.rs` | Trivial |
| 6 | `pub(super)` for `line_hash` | `src/tools/mod.rs` | Trivial |
