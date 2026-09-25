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

## Skill Sources

Skills are discovered from multiple locations, scanned in priority order:

| Source | Directory | Priority |
|--------|-----------|----------|
| Workspace | `skills/` | High |
| User Global | Extra directories from config (`[skills]` section) | Middle |

**Deduplication**: If multiple skills share the same name, the highest-priority source wins. Lookup is case-insensitive by name.

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
- Bundled skills (`residuum-system`, `residuum-getting-started`, `skill-authoring`) are written to `skills/` during workspace creation and follow the same format.
- Skill names must be unique across all sources. Workspace skills override user-global skills of the same name.

When a pattern keeps recurring across conversations, the agent is expected to author a new workspace skill itself rather than re-explaining the same instructions every time. Before authoring or editing a skill, activate the bundled **`skill-authoring`** skill — it holds the full doctrine (create-vs-patch decision, class-level shape, what not to capture, description-length limits) and is not repeated here.
