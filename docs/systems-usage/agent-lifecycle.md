# Agent Creation, Deletion and Restore

An agent is a directory `~/.residuum/<name>/` holding `config/config.toml`. Creating an agent writes that directory from the blank template and adds the agent's role page to the team wiki; deleting one checkpoints it, removes the directory, and removes the role page; restoring one brings the directory and the role page back from its checkpoint history. The functions in `src/hub/provision.rs` do the filesystem work and touch nothing else: they start and stop no agents and publish no notices, so the caller stops the agent before deleting it.

## Creating an agent

`provision_agent` takes a name, the full text of `providers.toml`, an A2A visibility, and an optional description.

1. The name must follow the agent-name rules (1-24 characters from `[a-z0-9-]`, no leading or trailing hyphen, not `hub`, `team` or `agents`). A name that breaks them is refused, and so is a name whose directory already exists.
2. `providers.toml` is validated against the hub's config the same way the agent's Settings save validates it. A file that isn't valid TOML, or assigns a model that can't be resolved, is refused before anything is written.
3. The directory is written from the blank template:
   - `SOUL.md`, the bundled default with the agent's name;
   - `HEARTBEAT.yml`, the built-in pulses without `wiki_lint` (the first agent owns that one);
   - `SUBCONSCIOUS.md`, `memory/OBSERVER.md`, `memory/REFLECTOR.md` and `config/agent-card.json`, the bundled defaults;
   - the standard empty directories (`memory/`, `skills/`, `inbox/`, `archive/`, `a2a/`);
   - the reference files `config/config.example.toml` and `config/providers.example.toml`;
   - `config/providers.toml`, exactly the text given;
   - `config/config.toml` with `autostart = true` and `[a2a] visibility`; every other value is the default.
4. There is no `BOOTSTRAP.md`. The `.bootstrapped` marker is written, so the first-run interview never runs, and starting the agent later does not recreate `BOOTSTRAP.md`.
5. The agent's role page `team/wiki/agents/<name>.md` is written, with the description as its one-line role (or the placeholder when there is none), together with its line in `team/wiki/agents/index.md` and an entry in the wiki log.

The files are assembled under a hidden staging directory, `~/.residuum/.provision-<name>/`, with `config/config.toml` written last, and the directory is renamed into place only when complete. An interrupted or failed creation therefore never leaves a discoverable half-agent: discovery ignores names starting with `.`, and the next creation of the same name clears a leftover staging directory. If a step fails, everything written is removed, including the directory when only the role page failed, and the error says nothing was created.

`copy_providers_from` reads an existing agent's `providers.toml`, for creating an agent with the same model configuration. `first_message` builds the text of the new agent's first message from its description: it asks the agent to turn the description into its own notes in `SOUL.md` and to fill in its role page (a one-line `description`, then role, responsibilities, and what teammates should hand it).

## Deleting an agent

`deprovision_agent` takes the agent's own checkpoint engine (workspace root = the agent's directory, config root = its `config/` directory) and does, in order:

1. Checkpoints the agent's directory in its workspace repository and its `config.toml` and `providers.toml` in its agent-config repository. It returns the workspace checkpoint id. When the workspace checkpoint can't be recorded it returns no id, logs a warning, and deletes anyway.
2. Renames the directory to `~/.residuum/.deleting-<name>/`, which takes the agent out of discovery in one step, and removes it.
3. Removes the role page and the agent's line in `team/wiki/agents/index.md`, and appends a removal entry to `team/wiki/log.md`, under the same lock that role-page creation uses.

On Windows a directory can't be renamed while a program has a file under it open, and the programs that do (a virus scanner, an indexer, one of the hub's own readers) let go within moments. When the rename is refused for that reason, the delete tries it again every 50 milliseconds for up to five seconds. It logs a warning when it starts to wait and an info line when the directory has moved. A directory still held after that fails the delete with a message saying another program has its files open: the agent is stopped and keeps its files, and the delete can be repeated.

An unknown agent is reported as not found. Checkpoints are never pruned, so the checkpoint stays available.

The host adds one file to that history. Before the role page goes, it reads the page's text, and after the deletion it writes `hub/checkpoints/agents/<name>/deleted.json` beside the agent's repositories, holding when the agent was deleted, the workspace checkpoint the deletion returned, and the role page's text. The record is what makes the deletion time and the default restore point exact: the newest checkpoint in the history can be older than the deletion (an unchanged workspace reuses the existing tip) or newer (a turn-end checkpoint still in flight when the agent stopped can land after the directory is gone), and the role page lives in the team folder, outside every agent repository. Creating or restoring the agent again removes the record.

## Restoring a deleted agent

`restore_agent` brings an agent back from a workspace checkpoint, using the agent's own checkpoint engine. It restores the whole workspace tree from the workspace repository, then `providers.toml` and `config.toml` from the latest checkpoint of the agent-config repository, `config.toml` last, so the agent is discoverable only once it is complete. It recreates the role page from the text the deletion record holds, with its roster entry and a log line, or with the placeholder role when there is no record. It refuses when the agent exists again, and can be retried after a failure. It starts nothing.

The host's restore wraps it. The user restores from the team view (the Undo on a deletion, or the Recently deleted list), with `residuum agent restore <name> [--checkpoint <id>]`, or with `POST /api/hub/agents/restore` (see [Hub HTTP Surface](hub-http.md#hub-routes)). Agents have no restore tool: a teammate deleted by an agent is restored by the user.

1. The name must follow the agent-name rules. Nothing restores once the hub is shutting down.
2. Under the same lock as creation, the name must be free: no agent on disk under it and none the host still holds (a delete of the same name that hasn't finished counts). Otherwise the answer is `AlreadyExists`. A name with no checkpoint history is `NoDeletedAgent`.
3. The checkpoint is the one asked for, which must exist in the agent's workspace history (otherwise a bad request), or the default: the checkpoint the deletion record names, else the newest checkpoint the deletion itself took, else the newest one.
4. The agent is restored, its deletion record is removed, and the host adopts it and starts it when its `autostart` is on. A restored agent whose start fails is returned `failed`, like a created one, and the reason is logged at info with the agent's name and state. A create whose start does not happen logs the same line.
5. The host publishes `agent_restored` naming who restored it, and files an item in the acting agent's user inbox when an agent did.

Restoring holds the same locks as creating, so a restore and a create of one name can't both write the directory: one wins and the other gets `AlreadyExists`. A restore and a delete of the same name are ordered the same way.

`--checkpoint` restores the agent's files from that workspace checkpoint. Its settings always come from the tip of its agent-config repository, and its role page from the latest deletion record, whichever checkpoint the files come from.

### The deleted list

`GET /api/hub/agents/deleted` and `residuum agent deleted` list the names that have checkpoint history under `hub/checkpoints/agents/` and no agent: no `config/config.toml` on disk and none the host holds. Each entry has the deletion time (from the deletion record, else the time of the newest checkpoint), and the checkpoint a restore uses by default. Newest deletion first. An agent that was restored or created again is not listed.

### Creating an agent with a deleted agent's name

Checkpoint history is keyed by agent name, so a new agent with the name of a deleted one continues the same history. Nothing is lost or rewritten: the deleted agent's checkpoints stay, the new agent's checkpoints follow them, and the checkpoint view of the new agent lists both. The deletion record is removed, so the name is no longer in the deleted list. When the new agent is deleted in turn, it is the one the list offers and the default restore returns; the earlier agent's files are still one `--checkpoint <id>` away.

## Agent tools

Every agent, and every session it forks, has two tools that call the hub as that agent. There is no approval step: the user sees the result as a toast, and the deleted toast carries an Undo that restores the agent.

- `agent_create` takes `name` and an optional `description`. The new agent copies the creator's `providers.toml` and takes the creator's A2A visibility. The result gives the new agent's name and state and says to reach it at `agent:<name>`. A creation whose start-up fails still creates the agent: the result says it failed to start, gives the reason, and points at the user's team view, where the user can fix and start it. A name that breaks the rules or is taken comes back as a tool error saying so.
- `agent_delete` takes `name`. The result gives the checkpoint id and says the user can restore the agent from the team view or with `residuum agent restore <name>`; the tool's description says agents have no restore tool. An unknown name comes back as a tool error.

The hub publishes `agent_created` or `agent_deleted` naming the acting agent, which the web UI shows as a toast, and files an item in the acting agent's user inbox. The deletion's inbox item says how to restore the agent. An agent that deletes itself gets no inbox item, since its directory is gone. When an agent creates another with a description, the new agent's first message arrives from the creator as a teammate message, carrying the creator's outgoing hop count; it is refused at the hop limit like any other message, and the user sees a warning that the role description could not be delivered. A user-created agent's first message is the owner's own message at hop 0.

The creation or deletion runs to completion even if the calling turn is cancelled part-way.

An agent can delete itself. Deleting an agent stops it, and stopping an agent cancels the turn that is running, so a delete awaited inside that turn would be cut off between the stop and the removal. The tool therefore starts the delete on its own task and returns at once; the agent is stopped moments later, its directory is checkpointed and removed, and the hub publishes `agent_deleted` naming the agent as the actor. The turn ends when the agent stops, so the agent should do everything else first. If the delete fails, the user gets a warning notice naming the agent and the reason; the agent is left stopped.

See [Checkpoints](checkpoints.md) for what each repository holds and [Team Directory](team-directory.md) for role pages.
