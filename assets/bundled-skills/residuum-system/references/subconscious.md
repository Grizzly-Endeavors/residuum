# Subconscious

A small-model classifier that watches the main agent's conversation and
steers it when it drifts from its instructions — e.g. acknowledging a user
preference but not persisting it to `MEMORY.md`, or acting against a rule in
`SOUL.md`/`AGENTS.md`. Opt-in, disabled by default.

## When it runs

Only for user-visible (foreground) turns — background, wake, system, and
sub-agent turns are never watched, which bounds the feedback loop.

- **Mid-turn watch**: every `every_n_iterations` tool-loop iterations, a
  detached classifier may inject a live course correction for an urgent
  (`act`) finding.
- **End-of-turn triage**: after the turn completes, evaluates the whole turn
  plus anything the mid-turn watch queued. The first `act` finding becomes a
  correction turn; `note` findings surface as passive `[Subconscious note]`
  context on the next turn.

## SUBCONSCIOUS.md

User-editable policy file (in the workspace) controlling *what* the
classifier checks for — read fresh from disk on every evaluation. The output
schema and "stay quiet unless certain" guidance are fixed by code, not
editable via the file.

## Learning

End-of-turn triage can also emit a `learn` signal (`preference` or
`recovery`), which spawns the `learner` sub-agent preset, subject to a
cooldown. Opt-in via `[subconscious] learning = true`. A separate
`[learning] nudge_after_turns` fallback exists for users who keep the
subconscious itself off.

## Config

`[subconscious]` in `config.toml`: `enabled` (default `false`), `mid_turn`,
`every_n_iterations`, `max_interventions_per_turn`, `max_transcript_tokens`,
`learning`, `learning_cooldown_minutes`. Model assigned via the
`subconscious` role in `providers.toml`.

See the authoritative reference: `docs/systems-usage/subconscious.md`.
