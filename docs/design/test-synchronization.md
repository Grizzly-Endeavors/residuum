# Test synchronization: wait on events, not elapsed time

**Status:** in progress. Tracking issue: #488.

## Problem

Tests flake under load because they wait for a length of time instead of for the thing they check. Debounces, batch windows, retries and detached background tasks run on the real clock, and few of them say "ready", "idle" or "done", so a test can only guess how long to wait. Most async tests run a whole hub on one current-thread runtime against that real clock. Load is the normal condition now (parallel agents, concurrent pre-commit runs, shared CI runners), so the guesses fail one at a time, and widening one guess leaves the rest in place. Fixes that waited on a real signal ended their flake outright; fixes that widened a duration did not.

## Approach

- **Measure under load.** `just stress` (`scripts/stress-tests.sh`) runs every test binary as two concurrent copies with CPU burners alongside, which reproduces how the suite runs when several agents work on one machine. Once the backlog below is empty, CI runs it as a required step of the Rust test job.
- **Tests wait on events.** A crate-level test toolkit (`src/testing/`) gives one hang guard (the suite's only deadline, there only to stop hung tests), event waits over channels and watches, a paused-clock helper set, a `Gate` for holding a call open until the test releases it, and a gated model server in front of wiremock.
- **A guardrail.** A source-scan test fails when test code calls `sleep`, `sleep_until`, `timeout`, `timeout_at` or wiremock's `set_delay` outside the toolkit. A per-file ratchet (`src/testing/wall_clock_backlog.rs`) lists the existing waits; counts can only go down.
- **Production signals.** Where a test needs to know that background work is ready, done or caught up, the code exposes it as product state users and diagnostics can see: the agent's live-update health in its status, checkpoint writes still in flight (which shutdown also waits for), push delivery outcomes and a cancellable retry, config pollers that take their baseline before spawning, and a `ping`/`pong` on the hub socket.
- **Web e2e.** Size the e2e runners so guaranteed CPU covers the workers, and give specs control of the mock's time (a manual time mode alongside turn holds) instead of racing scaled real-time delays.

## Phases

| Phase | Work | Model |
|---|---|---|
| R1 | Stress harness, loud watcher fallback, baseline below, process rules | Opus |
| R2 | `src/testing/` toolkit, source-scan classifier and guard, ratchet, "Waiting in tests" docs | Opus |
| R3 | Production signals (after #486) | Opus |
| R4 | Convert the backlog, area by area: hub; background and agent; gateway; notify, bus, interfaces, tools; a2a; tunnel, remote access and Pebble; the rest | Haiku batches, Opus review |
| R5 | Required Stress step in CI once the backlog is empty and `just stress` passes five rounds on `main` | Opus |
| W1 | e2e shard CPU throttling report, then runner sizing in grizzly-platform | Opus |
| W2 | Mock manual time, then convert specs that observe a time-bound window | Opus engine, Haiku specs |
| W3 | Vitest wall-clock asserts, real sleeps and `shouldAdvanceTime` | Haiku (#467 Opus) |

## Barrier patterns

- **Hold a state open:** a closed `Gate`, `until_held(1)`, act, then open it.
- **Let background work settle:** wait on the production signal for it; after a `watch_workspace`, a `ping` → `pong`.
- **"Nothing happened":** only after a positive sync point. A bus barrier (the broker fans out synchronously, so once the test's own subscriber has X every subscriber has it), a sentinel file written after the cause (inotify delivers in order), a stop (the watcher drains before `stop` returns), a pending count at zero, or `clock::within` on a paused clock for in-memory code.
- **Polling:** `testing::wait::until`.
- **Short positive timeouts:** `testing::wait::next` or `guarded`, under the hang guard.

## Baseline

Five rounds of `just stress` (two copies, 16 burners, 16 CPUs, load 27–37) on `main` at `c373b113` plus the R1 harness: each test ran 10 times. inotify instances peaked at 78 of 128, so no watcher fell back to polling. The memory search flake (#464) didn't recur; its tests already print anyhow's full error chain through `unwrap`, so the next failure shows tantivy's underlying error.

- [ ] 4/10 `hub::host::tests::main_conversation::a_background_turn_is_announced_with_its_visibility_live_and_in_history`: reads the chat history right after the turn's frames, before the background turn is recorded ("history records the background turn"). Needs a sync point on the history write.
- [ ] 2/10 `hub::runtime::tests::the_hub_serves_its_agents_on_one_port_and_stops_them_on_shutdown`: `eventually_status`'s 20s deadline passed before the agent card answered.
- [ ] 2/10 `hub::runtime::tests::onboarding_a_hub_without_agents_starts_the_first_agent_without_a_restart`: "Address already in use" on its reserved gateway port, or the same 20s deadline.
- [ ] 2/10 `hub::runtime::tests::changing_the_gateway_port_rebinds_the_server_and_keeps_the_agents_running`: the same two failures.
- [ ] 1/10 `hub::runtime::tests::a_hub_config_reload_applies_a_changed_push_contact`: "Address already in use" on its reserved port. `reserve_port` drops its listener before the hub binds, so a concurrent process can still take the port in between; the hub should be handed a bound listener or bind port 0 and report it.
- [ ] 1/10 `hub::host::tests::main_conversation::a_web_message_sent_during_a_telegram_turn_joins_it_and_the_web_sees_it_end`: the web message arrived before the held model call (an 800ms `set_delay`) started, so it opened the turn instead of joining it. Needs a gate and `until_held(1)`.
