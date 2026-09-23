# Tools Directory

## Mandatory: Keep TOOLS.md in sync

`TOOLS.md` is the canonical reference for every tool's LLM-facing contract (name, description, input schema, output format, side effects).

**You must update `TOOLS.md` whenever you:**
- Add a new tool (new `impl Tool` block or new `*Tool` struct)
- Remove or rename a tool
- Change `fn definition()` — description, parameter names, types, required fields, or enums
- Change `fn name()` (the tool's identifier)
- Change observable output format or error messages
- Change side effects that the LLM should reason about (e.g. `FileTracker`, `PathPolicy`, gating)

**Update `TOOLS.md` in the same commit** as the Rust change. Never let them drift.

The file lives at `src/tools/TOOLS.md`.

## Mechanical checklist: adding a new tool

- Declare the module in `src/tools/mod.rs` (`mod foo;` or `pub mod foo;`).
- Add a `register_*` method for it in `src/tools/registry.rs`, then wire that
  call into whichever registry-building surface(s) should carry it. There are
  **two separate registration surfaces**:
  - the main agent's registry, built by `init_tool_registry` in
    `src/gateway/startup/tools.rs`
  - the sub-agent (session) registry, built by
    `ToolRegistry::build_subagent_registry` in `src/tools/registry.rs`

  **The rule: every tool registered for main is also registered for
  sessions, with the same config gating, except `switch_endpoint`** — the one
  documented exception, because it redirects main's background-turn output
  and is meaningless for a session. If a tool genuinely must stay main-only,
  add it to the allowlist next to `switch_endpoint` in both
  `build_subagent_registry`'s doc comment and the `MAIN_ONLY_TOOLS` constant
  in `src/gateway/startup/tools.rs`'s tests, with a comment explaining why —
  don't just leave it off silently.

  The reverse also happens: a tool that only makes sense for a session, never
  for main (e.g. one registered only for sessions started from a particular
  endpoint), goes in `SESSION_ONLY_TOOLS` next to `MAIN_ONLY_TOOLS` in the
  same test file, with the same kind of comment explaining why main doesn't
  get it. `SESSION_ONLY_TOOLS` is empty until the first such tool exists.

  `gateway::startup::tools::tests::session_registry_matches_main_minus_documented_allowlist`
  enforces both directions: it builds both registries from equivalent config
  (every optional tool gate turned on) and asserts the session registry's
  tool names equal main's, minus `MAIN_ONLY_TOOLS` and plus
  `SESSION_ONLY_TOOLS`. Forgetting to wire a new tool into one of the two
  surfaces, or into the matching allowlist, fails this test instead of
  drifting silently.
