---
name: skill-authoring
description: Author, extend, and maintain Residuum skills.
---

# Skill Authoring

This skill is the doctrine for creating and maintaining skills well. Activate it
before writing a new skill, extending one, or consolidating overlapping ones. It
covers when a skill is warranted, what must never be captured, and how to keep
the skill set small and class-level rather than a sprawl of narrow one-session
notes. For the full body-structure checklist and worked examples, read
[authoring-standards](references/authoring-standards.md).

## When to Author

Write a skill only when the knowledge is durable and reusable:

- A procedure has been re-derived from scratch a **third** time.
- A workaround for an external constraint you cannot fix (an API quirk, a
  third-party format, a platform limitation that will persist).
- A workflow the user explicitly taught and will expect again.

Do **not** author for:

- One-off task narratives ("how I did X this session").
- Transient errors or a failure that a durable code/config fix removes.
- Environment-dependent failures (a path that only exists on one machine).
- Anything that will be false next week.

If a real fix would make the skill unnecessary, make the fix instead.

## Prefer Editing Over Creating

Before creating a new skill directory, walk this order and stop at the first that
fits:

1. **Patch the skill that was active** when the gap appeared — it was already the
   right home and missed a case.
2. **Extend an existing related skill** — add a section or a Pitfall.
3. **Add a reference or template file** to an existing skill and point its body
   at it — most detail belongs in `references/`, not in a new skill.
4. **Only then create a new skill**, and only for a genuinely new topic.

Target shape: **few class-level skills**, each owning a topic, each with a
`references/` directory for detail. A long flat list of narrow, one-session
skills is the failure mode — it bloats the index the model scans and buries the
skill that actually applies.

## Do Not Capture (Poison Prevention)

A skill outlives the moment it was written. These harden into liabilities:

- **Negative capability claims** — "tool X doesn't work", "the API can't do Y".
  They get cited as fact for months and turn into refusals long after the thing
  was fixed. State what *does* work; never encode a dead end.
- **Environment-dependent failures** — specific to one host, one network, one
  moment. Not reusable, actively misleading elsewhere.
- **Secrets or tokens** — never. Reference where a credential lives, never its
  value.
- **Stale specifics** — version numbers, ticket IDs, dated paths — unless the
  skill's whole subject *is* that thing. Otherwise the skill rots silently.

## Frontmatter Discipline

- `name`: lowercase, hyphenated, matches the directory name.
- `description`: **one sentence, ≤60 characters**, stating the capability. No
  marketing words ("powerful", "comprehensive", "seamless"). This line is what
  appears in the skills index the model scans to decide what to activate — make
  it a precise trigger, not a pitch.

## Maintenance

- **Patch on contact.** When you activate a skill and find it outdated, wrong, or
  missing a case, fix it in the same session. An unmaintained skill is a
  liability — every stale line erodes trust in the whole set.
- **Consolidate periodically.** When two or three narrow skills overlap, merge
  them into one class-level skill with `references/` for the detail. See the
  consolidation procedure in the reference doc.
- Deactivate skills when the task is done to keep context clean.

## Before You Write

Read [authoring-standards](references/authoring-standards.md) with `read_file`
for the body-structure template, writing rules, the create-vs-patch decision
example, the bad/good skill comparison, and the consolidation procedure. Every
claim you put in a skill must be something you **verified**, not something
plausible.
