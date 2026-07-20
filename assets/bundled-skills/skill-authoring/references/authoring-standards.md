# Authoring Standards

The full checklist for writing a skill body once you have decided (per the parent
SKILL.md) that a new or extended skill is warranted. Only `SKILL.md` is injected
on activation, so the body must be self-contained enough to act on, and must
point at `references/` files for anything long.

## Body Structure

A SKILL.md body should follow this shape. Omit a section only when it genuinely
does not apply — do not pad.

1. **Title** — `# Skill Name`, matching the `name` frontmatter.
2. **Intro (2–3 sentences)** — state what the skill does *and what it does not
   do*. The boundary matters as much as the capability: it stops the skill from
   being activated for adjacent work it does not cover.
3. **When to Use** — concrete triggers. The reader is deciding whether to
   activate; give them recognizable conditions, not abstractions.
4. **Prerequisites** — tools, active project, files, or credentials that must be
   in place first. Reference credentials by location, never by value.
5. **Procedure** — numbered, imperative steps. Each step is one action with a
   verifiable outcome.
6. **Pitfalls** — the failure modes you actually hit, and how to avoid each.
7. **Verification** — how to confirm the procedure worked. A skill that cannot be
   checked cannot be trusted.

Target length: **~100 lines** for a simple skill, **~200 lines** for a complex
one. Past that, move detail into `references/` and point at it. The body is
injected into the prompt on every activation — every line costs context.

## Writing Rules

- **Imperative voice.** "Run `cargo test --quiet`", not "you should run" or "the
  agent runs". The reader *is* the agent.
- **Real tool names.** Reference Residuum tools by their actual names —
  `read_file`, `write_file`, `edit_file`, `skill_activate`, `memory_search`,
  `subagent_spawn`. Do not invent or paraphrase tool names.
- **Scripts and templates live in the skill directory.** A helper script or a
  reusable template goes in the skill folder and is referenced by relative path
  (`read_file` on `templates/foo.md`), never pasted inline and never repeated. If
  you find yourself inlining the same block twice, extract it to a file.
- **Detail goes in `references/`.** The body names the reference and says when to
  read it; the reference holds the depth. Mirror how `residuum-system` points its
  quick-reference table at each `references/*.md`.
- **Every claim is verified.** Do not write what is plausible — write what you
  confirmed by running it, reading the code, or watching it work. An unverified
  claim in a skill is repeated with authority for months.

## Create-vs-Patch: Worked Example

You just spent a session working out that syncing a project's notes requires
activating the project *before* calling `memory_search`, or the search misses
project-scoped observations. You want to capture it.

- **Wrong:** create `project-memory-sync-ordering`. It is one narrow fact, it
  will sit alone in the index, and nobody scanning descriptions will guess it
  covers this.
- **Right:** the `residuum-system` memory reference already owns "how memory
  search works". Add the ordering rule there as a Pitfall. It lands where a
  reader already looks, and the index stays lean.

Rule of thumb: if the fact belongs to a topic an existing skill already owns,
patch that skill. Create only when the topic itself is new.

## Bad vs Good Skill

**Bad — narrow, session-specific, unverifiable:**

```yaml
---
name: fix-telegram-503-on-tuesday
description: How I got around the Telegram 503 error I saw last Tuesday.
---
# Fix Telegram 503
Last Tuesday sending failed with 503. I retried and it worked.
The API might be down sometimes. Retrying seems to help.
```

This encodes a transient failure, a dated specific, a maybe ("might", "seems"),
and no verification. It will never apply cleanly again and quietly misleads.

**Good — class-level, referenced, actionable:**

```yaml
---
name: notification-delivery
description: Send and retry Residuum notifications reliably.
---
# Notification Delivery
Covers sending via `send_message`, endpoint selection, and retry handling.
Does not cover channel configuration — see the residuum-system notifications
reference for that.
## Pitfalls
- Transient 5xx from an upstream channel is expected. Retry with backoff per
  references/retry-policy.md; do not treat one 5xx as an outage.
...
```

One skill owns the whole topic, states its boundary, cites a reference for the
retry detail, and makes a checkable claim.

## Consolidation Procedure

When narrow skills overlap, merge them into one class-level skill:

1. **Pick the survivor** — the name that best describes the *class*, not one
   instance. Prefer the broader, cleaner name.
2. **Absorb content** — fold each other skill's unique, still-true material into
   the survivor's body or its `references/`. Drop anything stale or negative
   during the move; consolidation is also a cleanup pass.
3. **Reconcile descriptions** — rewrite the survivor's frontmatter `description`
   to cover the merged scope, still one sentence and ≤60 characters.
4. **Leave nothing orphaned** — delete the absorbed skill directories and
   `grep` the workspace for any body that referenced them by name, updating those
   pointers to the survivor.
5. **Verify** — activate the survivor and confirm the merged procedure reads
   coherently end to end, with no dangling references.
