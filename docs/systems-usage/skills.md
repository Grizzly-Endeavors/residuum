# Skills

Skills are loadable instruction modules that inject specialized knowledge into the agent's context when activated. They are markdown files with YAML frontmatter — the body is injected verbatim into the system prompt when a skill is active.

## SKILL.md Format

```yaml
---
name: code-review
description: Structured code review workflow with security and performance checklists
---

(Markdown body — injected verbatim when activated)
```

Required frontmatter: `name` and `description`. The description is shown in the skill index so the agent can decide when to activate a skill.

## Skill Sources

Skills are discovered from multiple locations, scanned in priority order:

| Source | Directory | Priority |
|--------|-----------|----------|
| Project | `projects/<name>/skills/` (only when project active) | Highest |
| Workspace | `skills/` | High |
| User Global | Extra dirs from `[skills]` config section | Middle |
| Bundled | Shipped with the binary | Lowest |

If multiple skills share the same name, the highest-priority source wins. Lookup is case-insensitive by name.

## How Skills Appear in Context

- **Available skills**: listed in an `<available_skills>` block with name and description. The agent always sees this index and can decide to activate skills based on the current task.
- **Active skills**: full body injected in `<active_skill name="...">` blocks.

**Only SKILL.md is injected.** A skill directory may contain additional files (subdirectories, reference docs, workflow guides, etc.), but these are not automatically loaded. The SKILL.md body should instruct the agent to read those files using `read_file` when needed. For example, `residuum-getting-started` has a `workflows/` subdirectory — the SKILL.md body tells the agent which workflow file to read based on the user's goal.

## Tools

| Tool | Parameters | Notes |
|------|-----------|-------|
| `skill_activate` | `name` | Loads skill body into active context. Also triggers a rescan (removes active skills whose source files no longer exist). |
| `skill_deactivate` | `name` | Removes skill body from context. |

## Bundled Skills

Two skills are bundled with every workspace:

- **`residuum-system`**: Quick reference for all systems — tool names, config files, workspace layout. The agent activates this when it needs to look up operational details.
- **`residuum-getting-started`**: First-conversation onboarding. Routes the user into one of several guided workflows. Deactivates itself after the first conversation.

Bundled skills live under `skills/` in the workspace and follow the same format. They are written once during workspace creation and are not overwritten if the user (or agent) edits them.

## Intended Usage

Skills are the primary way to extend the agent's capabilities without changing code. Examples:

- Domain-specific workflows (deployment checklist, code review process)
- Integration guides (how to use a specific API or service)
- Knowledge packs (reference material for a framework or tool)
- Behavioral modes (different interaction styles for different contexts)

The agent should activate skills when it recognizes a task that matches, and deactivate them when the task is complete to keep context clean.

## Project Skills

Project-scoped skills are only visible while that project is active. Deactivating the project removes them from the skill index. This is useful for project-specific workflows that shouldn't clutter the global skill list.
