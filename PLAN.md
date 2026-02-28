# Two-Developer Parallel Work Plan

Two developers (A and B) working simultaneously with minimal idle time and zero merge conflicts. Each sprint is a merge point — both developers land their work before the next sprint starts.

---

## Sprint 1 — Immediate (no dependencies)

Both start at the same time. All items touch completely disjoint files.

### Developer A: Groups 1a + 4 + 9
1. **#13** Remove vestigial script execution (`background/script.rs`, `background/types.rs`, `background/mod.rs`)
2. **#16** Remove `hooks/` from workspace layout & bootstrap (`workspace/layout.rs`, `workspace/bootstrap.rs`)
3. **#20** Fix skill priority order (`skills/index.rs`, `skills/types.rs`)
4. **#22** Auto-load recent project logs on activation (`agent/context/loading.rs`, `agent/context/assembly.rs`, `projects/activation.rs`)

### Developer B: Groups 1b + 3
1. **#18** Add `tracing::warn!` for zero-channel notification routing (`notify/router.rs`)
2. **#14** Fix `memory_search` source filter values (`tools/memory_search.rs`, `memory/search.rs`)
3. **#12** Persist pulse `last_run` timestamps to disk (`pulse/scheduler.rs`, `pulse/types.rs`, `workspace/layout.rs`)
4. **#11** Add trigger count option for heartbeat pulses (`pulse/types.rs`, `pulse/scheduler.rs`, `pulse/executor.rs`)

**Merge point.** Both land to `main`.

---

## Sprint 2 — After Sprint 1

Group 1 is done, unlocking Group 2. Group 3 is done, reducing conflict surface for later Groups.

### Developer A: Group 2 (background/subagent)
1. **#17** Full transcript capture — serialize `recent_messages` (`background/subagent.rs`, `background/spawner.rs`, `agent/recent_messages.rs`)
2. **#24** Remove `wait` parameter from `subagent_spawn` (`tools/background.rs`, `tools/TOOLS.md`)
3. **#15** Include active skills in sub-agent context (`background/spawn_context.rs`, `background/subagent.rs`, `agent/context/assembly.rs`)

### Developer B: Group 5 (CLI/UX) + Group 7.3
1. **#1** CLI onboarding: debug log, `ironclaw logs`, welcome message (`main.rs`, `channels/cli/`)
2. **#2** CLI config/onboarding wizard (`config/bootstrap.rs`, `channels/cli/`)
3. **#3** Disallow LLM from editing config files (`gateway/server/mod.rs`, `tools/path_policy.rs`, `tools/write.rs`, `tools/edit.rs`)

**Merge point.**

---

## Sprint 3 — After Sprint 2

CLI and background work are done. Config and notification refactoring can begin.

### Developer A: Group 7.4+19 + Group 8.21
1. **#4** Improve secret handling (`config/resolve.rs`, `config/types.rs`, `config/deserialize.rs`)
2. **#19** Config internals cleanup (`config/*`) — builds on #4's changes
3. **#21** Model failover chain (`models/factory.rs`, `models/retry.rs`, `config/types.rs`, `config/resolve.rs`) — starts after #19 lands since both touch `config/`

### Developer B: Group 8.10 + Group 6 start
1. **#10** HTTP/SSE transport for MCP servers (`mcp/` — self-contained, no conflicts)
2. **#23** Rename NOTIFY.yml → CHANNELS.yml, move pulse routing to HEARTBEAT.yml (`notify/*`, `pulse/types.rs`, `workspace/layout.rs`, `workspace/bootstrap.rs`)

**Merge point.**

---

## Sprint 4 — After Sprint 3

Notification architecture refactoring continues. Config is stable.

### Developer A: Group 6 continued
1. **#8** Differentiate internal vs external channels (`notify/`, `channels/`)
2. **#9** `send_message` tool (`tools/` new file, `notify/router.rs`, `channels/`)
3. **#5** Unified slash command interface (`channels/`, `gateway/server/`)

### Developer B: Group 6 tail + cleanup
1. **#6** Improve inbox/external channel interface (`channels/`, `inbox/`)
2. **#7** `/inbox` command for Discord (`channels/discord/`)
3. Pick up any items that slipped or needed rework from earlier sprints

**Merge point.**

---

## Sprint 5 — OAuth (lowest priority, both developers)

Depends on #21 (model failover) from Sprint 3.

### Developer A: OpenAI Codex OAuth provider
- New provider in `models/`, OAuth flow module, Responses API format

### Developer B: Google Gemini CLI OAuth provider
- New provider in `models/`, OAuth flow module, Cloud Code Assist wrapper

### Then together: Anthropic OAuth overhaul
- Upgrade existing provider with full PKCE, token refresh, proper headers

---

## Timeline Shape

```
Sprint 1  ██████████  ██████████   (parallel, no deps)
                merge ↓
Sprint 2  ██████████  ██████████   (parallel, disjoint modules)
                merge ↓
Sprint 3  ██████████  ██████████   (A: config, B: MCP + notify rename)
                merge ↓
Sprint 4  ██████████  ██████████   (both: channel architecture)
                merge ↓
Sprint 5  ██████████  ██████████   (both: OAuth providers)
```

Neither developer waits for the other within a sprint. The only synchronization points are the merge points between sprints, where both land their branches before starting the next sprint.
