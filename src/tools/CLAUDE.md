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
