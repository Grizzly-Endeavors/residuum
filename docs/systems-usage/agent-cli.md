# Agent CLI

`residuum agent` manages the agents in a running hub. Each subcommand is an HTTP client of the hub's `/api/hub/agents` routes on the local gateway address, so the hub must be running (`residuum serve`). The hub owns the lifecycle; the CLI never edits agent directories itself. The routes and their bodies are defined in `docs/design/multi-agent-hub/http-contract.md`.

## Commands

| Command | Route | Output |
|---|---|---|
| `residuum agent list` | `GET /api/hub/agents` | A table of name, state, autostart and role. A failed agent gets an extra line with its last error and when it happened. |
| `residuum agent create <name> [--description <text>] [--models-from <agent>] [--public]` | `POST /api/hub/agents` | The new agent's state and visibility, and which agent its model settings were copied from. |
| `residuum agent delete <name>` | `DELETE /api/hub/agents/<name>` | The checkpoint id taken before removal. No confirmation prompt. |
| `residuum agent start <name>` | `POST /api/hub/agents/<name>/start` | The agent's state afterwards, plus its last error if it failed. |
| `residuum agent stop <name>` | `POST /api/hub/agents/<name>/stop` | Same. |
| `residuum agent restart <name>` | `POST /api/hub/agents/<name>/restart` | Same. |
| `residuum agent autostart <name> on\|off` | `PATCH /api/hub/agents/<name>` | Whether the agent starts when Residuum starts. |

## Create

- The name is checked locally with the same rules the hub applies (lowercase letters, digits and hyphens), so a bad name fails before any request.
- `--models-from <agent>` names the agent whose `providers.toml` is copied. Without it, the CLI copies from the only running agent. If none or several are running it stops with a message asking for `--models-from`.
- Visibility is private unless `--public` is given.
- `--description` becomes the new agent's first message and seeds its role page.

## Delete

Deleting checkpoints the agent's directory before removing it, and prints the checkpoint id. Checkpoints are never pruned, so the agent can be restored from the hub's checkpoint history. The CLI has no restore command; restore through the web UI's checkpoint history or the `workspace_restore` tool.

## Errors

- Hub not running: `Residuum isn't running. Start it with `residuum serve`.`
- `400`, `404` and `409` responses print the server's own `error` message (for example, no agent named 'x', or an agent that already exists).
- Any other failure prints the status code and points at `residuum logs`. Details go to the log as structured fields.

## Logs and stop

- `residuum logs --agent <name>` (`-a`) keeps only lines whose `agent` field, on the event or on any enclosing span, equals `<name>`. It composes with `--level`, `--module`, `--json` and `--watch`. Lines that aren't JSON can't be attributed to an agent and are dropped while the filter is active.
- `residuum stop` stops the whole hub and every agent in it through `POST /api/hub/shutdown`, falling back to SIGTERM. Use `residuum agent stop <name>` to stop one agent.

## Other commands that call the hub

`residuum tracing`, `residuum bug-report`, `residuum feedback` and the restart step of `residuum update` call the hub-level routes under `/api/hub/tracing/...` and `/api/hub/update/...`. `residuum secret`, `residuum agent-keys` and `residuum a2a keys` work on the hub's encrypted stores directly and need no running hub.
