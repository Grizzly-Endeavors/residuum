# Config Loading, Reload, and Startup Fallback

Residuum reads two files at startup and on every `/reload`: `config.toml` and `providers.toml`, both outside the workspace directory (see the systems-usage README's "User-owned files"). Loading degrades wherever it safely can instead of refusing to start over one bad setting — this page describes exactly what's fatal, what's skipped with a notice, and what happens when even the fatal path fails.

## Unknown keys

An unknown key in either file — one removed in a newer release, or a typo — is dropped and parsing retried, instead of failing the whole file. Every config struct still uses `#[serde(deny_unknown_fields)]`, so a typo is still caught; `config::tolerant::parse_tolerating_unknown_keys` finds the exact key or table `toml`'s parser flags, removes just that key (or, for a whole unrecognized table, that table's block) from the parsed text, and reparses. Each removed key produces a notice naming it and the file it was in. A type error on a *known* key (e.g. a string where a number is expected) is still fatal — only an unrecognized key is tolerated.

## Fatal vs. skip-with-notice

Most values that are missing or invalid fall into one of two buckets:

**Fatal — the gateway can't function without it:**
- No timezone configured.
- The main model spec can't be parsed, or its *primary* provider can't be built (missing API key, etc.) — see "Provider chains" below for what "primary" means here.
- `agent.max_tool_iterations = 0` (a turn that never calls a tool isn't a usable limit).
- A background model tier that can't be resolved.

**Skip-with-notice — an independent, optional feature degrades on its own:**
- A webhook entry (`[webhooks.<name>]`) with an invalid `routing`/`format` string, or an empty `content_fields` entry: that one webhook is dropped, the rest still load.
- `[teams]` present but missing `app_id`, `tenant_id`, or the app password: Teams is disabled, everything else starts normally.
- `idle.idle_channel` naming an unknown or unconfigured interface: idle switching stays disabled (falls back to no idle channel) rather than failing the config.
- `[a2a]` `visibility` set to anything other than `"public"`/`"private"`: falls back to the default visibility.
- The secret store (`secrets.toml.enc`) missing its key file, or failing to decrypt or parse: degrades to an empty store, so only entries that actually reference `secret:<name>` are affected (they resolve as if the secret were simply absent) — everything else in `config.toml`/`providers.toml` never touches the store and loads normally.

Every skip pushes a human-readable notice (`Config.load_notices`) naming what was skipped and why; the gateway publishes each one as a notice once it has a bus to publish to (see [Notifications](notifications.md)).

## Provider chains

`main`, `observer`, `reflector`, `pulse`, and `subconscious` are each a *chain*: the first (primary) provider, plus optional fallbacks tried in order if the primary's request fails at runtime (`inference::failover::FailoverProvider`). Chain resolution treats the primary and its fallbacks differently: an unbuildable fallback (a deleted `secret:` reference, a missing env key) is dropped from the chain with a notice, and the chain still builds from what's left. An unbuildable *primary* is fatal for `main` — that's the "no usable main provider" case described below — but the observer and reflector degrade independently of each other instead: if the observer's chain can't be built, only the observer falls back to a disabled stub (memory observation pauses) and the reflector still builds normally, and vice versa.

## Reload

`/reload` (or a `config.toml`/`providers.toml` change picked up by the file watcher) reloads the root config in place: it loads the new files, diffs them against the running config, and rebuilds only the subsystems that actually changed. A load failure — the new files don't parse or resolve — leaves the running config and files on disk untouched, and publishes a notice naming the error. A load that succeeds but changes nothing publishes a "no changes detected" notice. `channels.toml` (external notification channels) reloads on its own path alongside `mcp.json`; see [Notifications](notifications.md#channelstoml) for its parse-first behavior.

## Last-known-good fallback

After the gateway starts successfully, or a root reload fully applies, `config.toml` and `providers.toml` are copied (atomically) to `config.last-known-good.toml` and `providers.last-known-good.toml` in the same directory — `gateway::last_known_good`. These are gateway-owned housekeeping files, not something to hand-edit.

If startup hits a fatal problem — the live files fail to load, or they load but the gateway can't actually come up on them (no usable main provider, etc.) — the gateway loads the last-known-good copies instead, without touching the live files at all. If *that* succeeds, the gateway runs on the last-known-good config and publishes a notice once it's up, naming the problem in the live files and that it's running on the last configuration that worked; the web UI is reachable, so the fix is to edit `config.toml`/`providers.toml` and reload (or restart). If there's no last-known-good copy, or it fails too, startup fails with the original error, same as if there were no fallback.

Earlier versions kept `config.toml.bak`/`providers.toml.bak`, refreshed on every load attempt regardless of whether it worked. Those files are no longer written or read; if you still have them from an older install, they're unused and safe to delete.
