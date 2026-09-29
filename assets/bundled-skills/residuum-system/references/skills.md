# Skills

Skills are injectable instruction sets that extend agent capabilities. Each skill is a directory containing a `SKILL.md` file with YAML frontmatter and a markdown body.

## SKILL.md Format

```yaml
---
name: my-skill
description: Brief description shown in the skill listing.
---

# My Skill

Detailed instructions, workflows, and reference material.
The entire body below the frontmatter is injected into the system prompt when activated.
```

Editing a `SKILL.md` via `write_file`/`edit_file`, the workspace editor, or `POST /api/workspace/validate` reports invalid frontmatter YAML, a malformed `name`, or an empty/over-280-character `description` as a diagnostic alongside the save — the write always goes through rather than being rejected.

## Skill Sources

Skills are discovered from three layers, scanned in priority order:

| Layer | Directory | Priority |
|-------|-----------|----------|
| Agent | your own `skills/` | High |
| Team | `team/skills/`, shared by every agent in the hub | Middle |
| Configured | Extra directories from config (`[skills]` section) | Low |

Put a skill in your own `skills/` when only you should have it, and in `team/skills/` when every agent should.

**Deduplication**: If multiple skills share the same name, the highest-priority layer wins. Lookup is case-insensitive by name. The `<available_skills>` index shows each skill's layer in a `<layer>` element.

Skills used as session roles (`subagent_spawn`'s `skill`, a pulse's `agent`) resolve through the same layers, so a team skill works as a role.

A directory that can't be read (a permissions problem, not a missing directory) is skipped with a notice naming it, rather than discarding every skill already found in the other configured directories.

## Tools

| Tool | Parameters | Description |
|------|-----------|-------------|
| `skill_activate` | `name` (string) | Load a skill's body into the active system prompt. |
| `skill_deactivate` | `name` (string) | Remove a skill's body from the system prompt. |

## How Skills Appear in the Prompt

Available skills are listed in an XML block:

```xml
<available_skills>
- my-skill: Brief description shown in the skill listing.
- another-skill: Another description.
</available_skills>
```

Active skills inject their full body:

```xml
<active_skill name="my-skill">
# My Skill

Detailed instructions, workflows, and reference material.
</active_skill>
```

## Behavior

- **Activation** reads the SKILL.md body from disk and adds it to the active skill list. The body persists in the system prompt until deactivated.
- **Deactivation** removes the body from the prompt.
- **Rescan** (`skill_activate` triggers a rescan) re-reads all skill directories and removes any active skills whose source files no longer exist.
- Skill lookup is **case-insensitive** by name.

## Gotchas

- The skill body is injected verbatim — there is no templating or variable substitution.
- Bundled skills (`residuum-system`, `residuum-getting-started`, `skill-authoring`) are written to `team/skills/` when missing and follow the same format.
- Skill names must be unique across all layers. Your own skills override team skills, which override configured-directory skills, of the same name.

When a pattern keeps recurring across conversations, the agent is expected to author a new skill itself rather than re-explaining the same instructions every time. Before authoring or editing a skill, activate the bundled **`skill-authoring`** skill — it holds the full doctrine (create-vs-patch decision, class-level shape, what not to capture, description-length limits) and is not repeated here.
