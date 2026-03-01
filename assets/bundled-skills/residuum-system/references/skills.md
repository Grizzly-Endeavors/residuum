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
| Project | `projects/<name>/skills/` (only when project is active) | Highest |
| Workspace | `skills/` | High |
| User Global | Extra directories from config (`[skills]` section) | Middle |
| Bundled | Shipped with the binary (e.g., `residuum-system`, `residuum-getting-started`) | Lowest |

**Deduplication**: If multiple skills share the same name, the highest-priority source wins. Lookup is case-insensitive by name.

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
- Project skills only appear in the index while that project is active. Deactivating the project removes them from the index and deactivates any that were active.
- Bundled skills (like `residuum-system` and `residuum-getting-started`) are written to `skills/` during workspace creation and follow the same format.
- Skill names must be unique across all sources. Project skills override workspace skills of the same name, workspace overrides user-global, and so on down the priority chain.
