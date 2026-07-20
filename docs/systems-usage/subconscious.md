# Subconscious

The subconscious is a small-model classifier that watches the main agent's conversation and steers it when it drifts from its instructions — for example, when the agent acknowledges a user preference but forgets to persist it to `MEMORY.md`, or acts against a rule in `SOUL.md`/`AGENTS.md`. It is **opt-in** and disabled by default, because it adds a classifier call to every evaluated turn.

## When it runs

The subconscious evaluates in two phases, and **only** for user-visible (foreground) turns:

1. **Mid-turn watch** (optional, `mid_turn`): while the agent is working through its tool loop, a detached classifier task may run every `every_n_iterations` iterations. If it finds an urgent problem (`act`), a course correction is injected into the turn at the next interrupt checkpoint — the tool loop never blocks on the classifier.

2. **End-of-turn triage**: after a user-visible turn completes, the classifier evaluates the whole turn. It runs as a **triage step**, not a blind re-check: it receives the corrections the mid-turn watch already applied and any lower-urgency notes it queued, so it does not repeat steering and can fold queued notes into a final decision.

Background turns — including the subconscious's own correction turns — are **never** evaluated. That gate is what bounds the feedback loop: a correction can't trigger another evaluation.

Wake turns, system turns, and sub-agent turns are also never watched (loop prevention and scope: the subconscious watches the main agent only).

### Findings

Each finding the classifier reports has:

- **kind** — `violation` (acted against an instruction) or `omission` (failed to do something it should have).
- **severity** — `act` (needs correction now) or `note` (just worth knowing for next time).

How a finding is delivered:

| Phase | `act` | `note` |
|-------|-------|--------|
| Mid-turn | Injected as a live course correction (subject to `max_interventions_per_turn`); recorded for end-of-turn triage. | Not injected mid-turn; queued as triage input for the end-of-turn pass. |
| End-of-turn | The **first** `act` becomes an immediate correction turn (the agent wakes and takes the corrective action). Later `act` findings degrade to notes. | Injected as passive `[Subconscious note]` context the agent sees on its next turn. |

If the end-of-turn evaluation fails, any notes the mid-turn watch queued are delivered raw (as `[Subconscious note]` context) rather than being silently dropped.

## SUBCONSCIOUS.md

`SUBCONSCIOUS.md` in the workspace is the user-editable policy that controls **what** the classifier watches for. It is read fresh from disk on every evaluation, so edits take effect immediately without a restart. The agent can modify it at the user's request with standard file tools.

The file controls the *checks* only. The output format (the JSON findings schema) and the "stay quiet unless there's a clear problem" guidance are always injected by code and cannot be broken by editing the file. The default policy biases heavily toward silence: returning no findings is the normal, correct outcome for most turns, and the classifier is instructed never to fabricate a finding to seem useful.

The bundled default `SUBCONSCIOUS.md` watches for:

- A stated user preference, fact, or standing instruction the agent acknowledged (or should have noticed) but did not persist to `MEMORY.md`
- The agent directly contradicting a rule in `SOUL.md`, `AGENTS.md`, or `USER.md`
- The agent ignoring an explicit user request from earlier in the same conversation segment
- The agent claiming it did something the transcript shows it did not do
- The user asking what the agent can do (or whether something is possible) and the agent answering from guesswork without activating the `residuum-system` skill first

The user or agent can edit this list at any time — it's a plain workspace file, not baked into the binary.

The agent's own instruction files — `SOUL.md`, `AGENTS.md`, `USER.md`, `MEMORY.md` — are supplied to the classifier as the rules to check against. Missing files are skipped.

## Guards against over-steering

The subconscious is deliberately conservative:

- Mid-turn evaluations run at most one at a time (a slow classifier can't pile up) and are capped at `max_interventions_per_turn` corrections per turn.
- End-of-turn spawns at most one correction turn per turn.
- The prompt makes silence the expected result and forbids inventing findings.
- The end-of-turn triage pass sees what was already steered, so it won't repeat a correction in the same turn.

## Cost & latency

Enabling the subconscious adds LLM calls: up to one mid-turn call per `every_n_iterations` tool iterations, plus one end-of-turn call per user turn. Assign a cheap, fast model to the `subconscious` role in `providers.toml`.

The end-of-turn evaluation runs **synchronously on the gateway event loop** (like the observer), so it adds one classifier round-trip before the next inbound message is handled. It does not delay the user-visible reply, which has already been sent by the time it runs. The mid-turn watch, by contrast, runs in detached tasks and never blocks the turn.

## Configuration

`[subconscious]` in `config.toml` (all optional; the section's absence means disabled):

| Key | Default | Meaning |
|-----|---------|---------|
| `enabled` | `false` | Master switch. Opt-in. |
| `mid_turn` | `true` | Whether the mid-turn watch runs (only when `enabled`). |
| `every_n_iterations` | `3` | Mid-turn: evaluate every N tool-loop iterations. |
| `max_interventions_per_turn` | `1` | Mid-turn: cap on injected corrections per turn. |
| `max_transcript_tokens` | `12000` | Transcript budget sent to the classifier (oldest messages dropped to fit). |

The model is assigned via the `subconscious` role in `providers.toml`, following the standard `models.subconscious` → `models.default` → `main` precedence. All of these are also editable from the web Settings UI (Runtime and Providers panels).

Provider or config changes are applied on config reload: the subconscious is rebuilt, in-flight evaluations keep the old instance, and new turns use the new one. If the provider fails to build, the subconscious falls back to disabled rather than failing startup — the main agent keeps working without it.
