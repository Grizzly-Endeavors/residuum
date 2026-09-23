# A2A (Agent2Agent)

A2A is how other agents — including the user's own other Residuum instances — can reach you over a standard protocol (JSON-RPC/REST, spec v1.0.1). Residuum runs a dedicated listener for it, separate from the web UI's gateway port.

## What you control

`config/agent-card.json` is what you advertise to other agents that discover you over A2A: your name, description, and a list of skills (`id`, `name`, `description`, `tags`, optional `examples`). Edit it like any other workspace file — `write_if_missing` seeded it with a generic placeholder and no skills on first run, so an empty `skills` list is normal until you or the user fill it in. A skill `id` here that also names one of your workspace skills is meant to start an inbound A2A conversation with that skill; treat the file as the callers'-eye view of what you can do.

Keep `name` and `description` non-empty and every skill `id` unique — an invalid file makes the listener fall back to serving the last good card (or a minimal generic one, at startup) rather than your edit, so a broken edit doesn't quietly do nothing.

## What you don't control

- **Caller keys** (the credentials other agents present) are managed by the user with `residuum a2a keys create|list|revoke` or Settings → A2A. You have no tool for minting, listing, or revoking them.
- **Visibility** (`public` vs `private` in `[a2a]`) and the listener's port are config, set by the user.
- **Handling an inbound task.** The listener currently refuses every A2A request with an "unsupported operation" error — there is no session executor wired up to it yet. If asked whether you can receive tasks over A2A right now, say no rather than guessing; don't claim capability the system doesn't have yet.

See the authoritative reference: `docs/systems-usage/a2a.md`.
