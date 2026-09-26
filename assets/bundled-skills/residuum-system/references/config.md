# Editing config.toml and providers.toml

Write `config.toml` and `providers.toml` directly with `write_file`/`edit_file` when the user asks for a config change — both are writable. `config.example.toml` and `providers.example.toml` stay blocked: Residuum regenerates them from its own defaults on every startup, so any edit to them is lost at the next restart. Edit `config.toml`/`providers.toml` instead.

## Which file holds a setting

- `config.toml` — everything except providers and models: `[agent]`, `[background]`, `[subconscious]`, `[memory]`, `[gateway]`, `[discord]`/`[telegram]`/`[teams]`/`[a2a]`, `[web_search]`, `[tools]`, `[webhooks.<name>]`, top-level `timezone`/`timeout_secs`/`max_tokens`.
- `providers.toml` — provider credentials and `[models]` role assignments (`main`, `subconscious`, background tiers).
- Workspace `config/*.json`/`*.toml` (`mcp.json`, `channels.toml`, `agent-card.json`, `a2a.json`) — per-system files, not global settings. Read the matching reference (mcp.md, notifications.md, a2a.md) before editing one of these; they have their own formats.

## How to edit

1. Use `edit_file` with a targeted `old_string`/`new_string` that changes only the lines that need to change — both files carry comments and commented-out examples the user relies on, and a full `write_file` rewrite drops them.
2. If the key doesn't exist yet, add it under the matching `[section]` header (adding the header itself if the section is absent), placed near related keys rather than appended at the end of the file.
3. Match the file's existing style: comment out an example you're leaving as a hint, uncomment and set a value to enable it.
4. If the field you're editing already holds a `secret:<name>` or `${ENV_VAR}` reference, keep that reference form — edit around it rather than replacing it with a plaintext value. When the user says a credential is already stored as a secret, ask for its name and write `secret:<name>`; don't ask the user to paste the plaintext value into the conversation when a stored reference will do.
5. After the write succeeds, a background watcher picks up the change and reloads automatically — there is no tool-level confirmation of this in `write_file`/`edit_file`'s own result. The reload's outcome ("configuration reloaded: ..." or "config reload failed: ...") is published to the user's interface (a notice in the web UI, chat, etc.) *and*, because this write is yours, arrives as a system note in your own transcript on your next step — you don't need to ask the user to check for you. Before writing, re-read the edited section to make sure the TOML is well-formed (balanced quotes/brackets/tables, correct value types) — a malformed file fails the reload and Residuum keeps running on the previous config until it's fixed.
6. Tell the user what changed — name the section, the key, and the new value, not just "updated the config". Once the system note confirming the reload arrives, mention its outcome too rather than assuming success; if it reports a failure, say so and offer to fix it.
