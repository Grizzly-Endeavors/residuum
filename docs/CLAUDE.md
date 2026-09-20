# docs/

## Layout

| Directory | Holds | Trust |
|-----------|-------|-------|
| `systems-usage/` | How each system is intended to work, for both the agent and the user | **Authoritative.** Keep current with the code. |
| `guides/` | Task-oriented walkthroughs — how to accomplish something end to end | Current. |
| `design/` | Designs for work that is not built yet | Current intent, not current behavior. |
| `archive/` | Superseded documents, kept for history | Historical. Do not treat as a description of the system. |

`design-philosophy.md` sits at the top level: it describes durable principles rather than any particular implementation.

## Where things go

- A system's behavior changed → update the matching file in `systems-usage/`, and its mirror under `assets/bundled-skills/residuum-system/references/`.
- Writing up work that hasn't been built → `design/`. Move it to `archive/` once the work ships and `systems-usage/` describes the result.
- A document no longer describes the system → move it to `archive/` rather than annotating it.

Documents describe the system as it is now. Point-in-time content — migration narration, "this replaces", dated status notes — belongs in `archive/`, not in a live document.

## Other current references

- `assets/bundled-skills/residuum-system/references/` — bundled skill references, shipped in the binary and kept in sync with `systems-usage/`
- The code itself
