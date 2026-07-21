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
  **two separate registration surfaces** and a new tool is not on both by default:
  - the main agent's registry, built in `src/gateway/startup/tools.rs`
  - the sub-agent registry, built by `ToolRegistry::build_subagent_registry`
    in `src/tools/registry.rs`
  Decide on purpose whether the new tool belongs in one or both — do not assume
  parity between them. (Past gap: `ollama_web_search` is registered for the main
  agent in `gateway/startup/tools.rs` but has no corresponding call in
  `build_subagent_registry`, with no record of whether that's intentional.)
