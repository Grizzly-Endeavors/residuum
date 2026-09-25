# A2A (Agent2Agent)

A2A is how other agents — including the user's own other Residuum instances — can reach you over a standard protocol (JSON-RPC/REST, spec v1.0.1). Residuum runs a dedicated listener for it, separate from the web UI's gateway port. Each caller's task runs as its own conversation session, addressed by `{caller}/{context_id}` — the same session lifecycle any other conversation interface uses.

## What you control

`config/agent-card.json` is what you advertise to other agents that discover you over A2A: your name, description, and a list of skills (`id`, `name`, `description`, `tags`, optional `examples`). Edit it like any other workspace file — `write_if_missing` seeded it with a generic placeholder and no skills on first run, so an empty `skills` list is normal until you or the user fill it in. A skill `id` here that also names one of your workspace skills starts an inbound A2A session with that skill activated as its role, when the caller asks for it by name; treat the file as the callers'-eye view of what you can do.

Keep `name` and `description` non-empty and every skill `id` unique — an invalid file makes the listener fall back to serving the last good card (or a minimal generic one, at startup) rather than your edit, so a broken edit doesn't quietly do nothing.

**`a2a_task_update`** is the tool a session started from the `a2a` endpoint uses to report its task's outcome: `{state: "completed" | "input_required" | "failed", message, artifacts?}`. Call it when you're done, when you need more from the caller before continuing, or when you can't finish — `message` is the only thing the caller ever sees, so put your real answer there, not just a summary of what you did. A session that never calls it still resolves once its run ends (completed with its last reply, cancelled, or failed), but calling it explicitly is how you ask the caller a question (`input_required`) or attach artifacts.

If Residuum restarts while an A2A task is unfinished, the task's session receives a message saying so and repeating the caller's requests on that task. Finish that task (not any other) and report it with `a2a_task_update`.

## What you don't control

- **Caller keys** (the credentials other agents present) are managed by the user with `residuum a2a keys create|list|revoke` or Settings → A2A. You have no tool for minting, listing, or revoking them.
- **Visibility** (`public` vs `private` in `[a2a]`) and the listener's port are config, set by the user.
- **Who a task belongs to.** Every task is scoped to the caller that created it — you never see or act on another caller's task, even if you can see its address.

## Reaching other agents

You can also delegate to other agents over A2A with your ordinary tools — `list_agents`, `message_agent`, `stop_agent` — using the address `a2a:<name>`. `config/a2a.json` lists them (you can edit it directly), each with a `url` and optional `headers` (which can reference an agent key with `${agent-key:<name>}`).

- `list_agents` shows every configured remote agent's live status (online with its description and skills, still resolving, or unreachable) and any open tasks you have with it.
- `message_agent` to `a2a:<name>` sends a follow-up on your open task with that agent if one is waiting on you (`INPUT_REQUIRED`/`AUTH_REQUIRED`), otherwise starts a new one. It returns immediately — the reply is not synchronous. An optional `skill` parameter names one of the remote agent's advertised skills.
- The reply arrives later as an ordinary agent message from `a2a:<name>`, naming the task and its new state, once the task needs your attention (it asks a question, needs auth, or finishes). Don't wait for it inline; go on with other work and react when it lands.
- `stop_agent` on `a2a:<name>` cancels your open task with that agent.

Editing `config/a2a.json` via `write_file`/`edit_file`, the workspace editor, `POST /api/workspace/validate`, or the Settings page's raw editor reports invalid JSON, an invalid agent name, or an empty url as a diagnostic alongside the save — the write always goes through rather than being rejected.

## Your other instances (siblings)

If the user runs more than one Residuum instance, they find and trust each other automatically through the relay — no `config/a2a.json` entry or caller key needed. `list_agents` marks one with `(your instance)`; a message from it reads as coming from that instance by name (e.g. `laptop`), described as your own other instance rather than an external caller. Talk to it the same way as any other remote agent, with `a2a:<name>`.

See the authoritative reference: `docs/systems-usage/a2a.md`.
