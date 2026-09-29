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

Editing a `SKILL.md` through the agent's `write_file`/`edit_file` tools, the workspace editor, or `POST /api/workspace/validate` reports the same problems the skill scanner would reject: invalid YAML frontmatter (with the parser's line/column, when the failure is in the YAML itself), an invalid `name` (must be 1-64 lowercase alphanumeric-and-hyphen characters, no leading/trailing/consecutive hyphens), or an empty or over-280-character `description`. The save always goes through — a diagnostic names the problem instead of the write being rejected, since a skill that fails to parse this way is silently dropped by the scanner (a `tracing::warn!` log, no owner-facing notice) rather than causing a load-time failure.

## Skill Sources

Skills are discovered from three layers, scanned in priority order:

| Layer | Directory | Priority |
|-------|-----------|----------|
| Agent | the agent's own `skills/` | High |
| Team | `team/skills/`, shared by every agent in the hub | Middle |
| Configured | Extra dirs from the agent's `[skills]` config section | Low |

Put a skill in the agent's `skills/` when only that agent should have it, and in `team/skills/` when every agent should. A fresh agent's `skills/` starts empty.

If multiple skills share the same name, the highest-priority layer wins and the others are hidden (logged at debug with both paths). Lookup is case-insensitive by name.

Each indexed skill records the layer it came from. The `<available_skills>` block shows it in a `<layer>` element (`agent`, `team`, or `configured`).

The index is rebuilt at startup, after a reload of the config, and on `skill_activate`, so changes to `team/skills/` are picked up on the same triggers as changes to the agent's own `skills/`.

Skills used as session roles (`subagent_spawn`'s `skill`, a pulse's `agent`, artifact sessions, an inbound A2A `metadata.skill`) resolve through the same layered index, so a team skill works as a role for any agent.

A directory that can't be read (a permissions problem, not a missing directory) is skipped with a notice naming it, rather than discarding every skill already found in the other configured directories.

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

Three skills are bundled with the team:

- **`residuum-system`**: Quick reference for all systems — tool names, config files, workspace layout. The agent activates this when it needs to look up operational details.
- **`residuum-getting-started`**: First-conversation onboarding. Routes the user into one of several guided workflows. Deactivates itself after the first conversation.
- **`skill-authoring`**: The agent's doctrine for creating and maintaining its own skills — when to create a new skill versus patch an existing one, what shape a skill should take, what not to capture in a skill, and description-length discipline (descriptions stay under ~60 characters so the skill index stays scannable). The agent should activate this skill whenever it's about to author or edit a skill, rather than improvising the format.

Bundled skills live under `team/skills/` and follow the same format, so every agent sees them as team skills. They are written when missing and are not overwritten if the user (or an agent) edits them.

### Create vs. Patch

When a new instruction or workflow needs to live somewhere, the agent should default to extending an existing skill over spinning up a new one — a sprawl of narrow single-purpose skills is harder to discover and activate correctly than a smaller set of well-scoped ones. `skill-authoring` is the authoritative reference for the full create-vs-patch decision; this is a pointer, not a restatement.

## Intended Usage

Skills are the primary way to extend the agent's capabilities without changing code. Examples:

- Domain-specific workflows (deployment checklist, code review process)
- Integration guides (how to use a specific API or service)
- Knowledge packs (reference material for a framework or tool)
- Behavioral modes (different interaction styles for different contexts)

The agent should activate skills when it recognizes a task that matches, and deactivate them when the task is complete to keep context clean.
