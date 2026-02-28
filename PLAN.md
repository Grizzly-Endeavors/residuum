# Two-Developer Parallel Work Plan

Two AI developers (A and B) working simultaneously. Dev A works directly on `main`. Dev B works in the `worktree-backlog` worktree (`.claude/worktrees/backlog/`). Between sprints, the coordinator merges Dev B's branch into `main`, runs tests, and resets the worktree to the updated main.

## Sprint 1 — Immediate (no dependencies)

All items touch completely disjoint files. Both devs start at the same time.

### Dev A (main): Groups 1a + 4 + 9
1. **#13** — Remove vestigial script execution
2. **#16** — Remove `hooks/` from workspace layout and bootstrap
3. **#20** — Fix skill priority order
4. **#22** — Auto-load recent project logs on activation

### Dev B (.claude/worktrees/backlog/): Groups 1b + 3
1. **#18** — Warn on zero-channel notification routing
2. **#14** — Fix `memory_search` source filter values
3. **#12** — Persist pulse `last_run` timestamps to disk
4. **#11** — Add trigger count option for heartbeat pulses

**Merge → test → push.**

---

## Sprint 2 — After Sprint 1

Group 1 is done, unlocking Group 2 (background/). Group 3 is done, reducing conflict surface.

### Dev A (main): Group 2 (background/subagent)
1. **#17** — Full transcript capture for background tasks
2. **#24** — Remove `wait` parameter from `subagent_spawn`

### Dev B (.claude/worktrees/backlog/): Group 5 (CLI/UX) + Group 7.3
1. **#1** — CLI onboarding and logging
2. **#2** — CLI config/onboarding wizard
3. **#3** — Disallow LLM from editing config files

**Merge → test → push.**

---

## Sprint 3 — After Sprint 2

CLI and background work are done. Config and notification refactoring begin.

### Dev A (main): Group 7.4+19 + Group 8.21
1. **#4** — Improve secret handling
2. **#19** — Config internals cleanup (builds on #4)
3. **#21** — Model failover chain (starts after #19 since both touch `config/`)

### Dev B (.claude/worktrees/backlog/): Group 8.10 + Group 6 start
1. **#10** — HTTP/SSE transport for MCP servers
2. **#23** — Rename NOTIFY.yml → CHANNELS.yml, split pulse routing

**Merge → test → push.**

---

## Sprint 4 — After Sprint 3

Notification architecture continues. Config is stable.

### Dev A (main): Group 6 continued
1. **#8** — Differentiate internal vs external channels
2. **#9** — `send_message` tool
3. **#5** — Unified slash command interface

### Dev B (.claude/worktrees/backlog/): Group 6 tail
1. **#6** — Improve inbox/external channel interface
2. **#7** — `/inbox` command for Discord

**Merge → test → push.**

---

## Sprint 5 — OAuth (lowest priority)

Depends on #21 (model failover) from Sprint 3.

### Dev A (main): OpenAI Codex OAuth provider
- New provider in `models/`, OAuth flow module, Responses API format

### Dev B (.claude/worktrees/backlog/): Google Gemini CLI OAuth provider
- New provider in `models/`, OAuth flow module, Cloud Code Assist wrapper

### Then together: Anthropic OAuth overhaul
- Upgrade existing provider with full PKCE, token refresh, proper headers

**Merge → test → push.**

---

## Timeline

```
Sprint 1  ██ Dev A (main) ██  ██ Dev B (backlog) ██
                    merge ↓
Sprint 2  ██ Dev A (main) ██  ██ Dev B (backlog) ██
                    merge ↓
Sprint 3  ██ Dev A (main) ██  ██ Dev B (backlog) ██
                    merge ↓
Sprint 4  ██ Dev A (main) ██  ██ Dev B (backlog) ██
                    merge ↓
Sprint 5  ██ Dev A (main) ██  ██ Dev B (backlog) ██
```

Neither developer waits for the other within a sprint. The only synchronization is the merge + reset between sprints.
