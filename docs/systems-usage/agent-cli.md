# Agent CLI

`residuum agent` manages the agents in a running hub. Each subcommand is an HTTP client of the hub's `/api/hub/agents` routes on the local gateway address, so the hub must be running (`residuum serve`). The hub owns the lifecycle; the CLI never edits agent directories itself. The routes and their bodies are in [Hub HTTP Surface](hub-http.md).

## Commands

| Command | Route | Output |
|---|---|---|
| `residuum agent list` | `GET /api/hub/agents` | A table of name, state, autostart and role. A failed agent gets an extra line with its last error and when it happened. |
| `residuum agent create <name> [--description <text>] [--models-from <agent>] [--public]` | `POST /api/hub/agents` | The new agent's state and visibility, and which agent its model settings were copied from. |
| `residuum agent delete <name>` | `DELETE /api/hub/agents/<name>` | The checkpoint id taken before removal, and the command that restores the agent. No confirmation prompt. |
| `residuum agent deleted` | `GET /api/hub/agents/deleted` | A table of the deleted agents that can be restored: name, when it was deleted, and the checkpoint a restore uses. |
| `residuum agent restore <name> [--checkpoint <id>]` | `POST /api/hub/agents/restore` | The restored agent's state, plus its last error if it failed to start. |
| `residuum agent start <name>` | `POST /api/hub/agents/<name>/start` | The agent's state afterwards, plus its last error if it failed. |
| `residuum agent stop <name>` | `POST /api/hub/agents/<name>/stop` | Same. |
| `residuum agent restart <name>` | `POST /api/hub/agents/<name>/restart` | Same. |
| `residuum agent autostart <name> on\|off` | `PATCH /api/hub/agents/<name>` | Whether the agent starts when Residuum starts. |

## Create

- The name is checked locally with the same rules the hub applies (up to 32 characters: letters, numbers, spaces, hyphens, and apostrophes), so a bad name fails before any request. A name can also be the folder name.
- `--models-from <agent>` names the agent whose `providers.toml` is copied. Without it, the CLI copies from the only running agent. If none or several are running it stops with a message asking for `--models-from`.
- Visibility is private unless `--public` is given.
- `--description` becomes the new agent's first message and seeds its role page.

## Delete and restore

Deleting checkpoints the agent's directory before removing it, and prints the checkpoint id and `residuum agent restore <name>`, the command that undoes it. Checkpoints are never pruned.

`residuum agent deleted` lists the agents that can be restored. `residuum agent restore <name>` brings one back with its files, settings and role page, and starts it when its `autostart` is on. `--checkpoint <id>` restores its files from that workspace checkpoint instead of the one the deletion took (the ids are in the agent's checkpoint history); its settings and role page are still the ones it was deleted with. A name that exists is refused (`409`) and a name with no history is `404`; both print the server's message.

Checkpoints are keyed by the folder name. Creating an agent with the same name, ignoring case, continues that history. A different name that would use the same folder gets the next folder (`research-desk-2`) and its own history. See [Agent Creation, Deletion and Restore](agent-lifecycle.md#creating-an-agent-with-a-deleted-agents-name).

## Errors

- Hub not running: `Residuum isn't running. Start it with `residuum serve`.`
- `400`, `404` and `409` responses print the server's own `error` message (for example, no agent named 'x', or an agent that already exists).
- Any other failure prints the status code and points at `residuum logs`. Details go to the log as structured fields.

## Logs and stop

- `residuum logs --agent <name>` (`-a`) keeps only lines whose `agent` field, on the event or on any enclosing span, equals `<name>`. It composes with `--level`, `--module`, `--json` and `--watch`. Lines that aren't JSON can't be attributed to an agent and are dropped while the filter is active.
- `residuum stop` stops the whole hub and every agent in it through `POST /api/hub/shutdown`, falling back to SIGTERM. Use `residuum agent stop <name>` to stop one agent.

## Other commands that call the hub

`residuum tracing`, `residuum bug-report`, `residuum feedback` and the restart step of `residuum update` call the hub-level routes under `/api/hub/tracing/...` and `/api/hub/update/...`. `residuum secret`, `residuum agent-keys` and `residuum a2a keys` work on the hub's encrypted stores directly and need no running hub.
