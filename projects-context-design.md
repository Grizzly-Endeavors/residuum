# Personal AI Agent — Projects Context Management

## Overview

This document describes a context management system that gives the agent structured, scoped knowledge it can activate and deactivate autonomously. Each project is a self-contained, self-describing context folder with its own configuration, agent notes, reference material, and a working directory.

This system is independent of but complementary to the Memory & Proactivity design. OM handles *what happened* (episodic continuity). The Projects system handles *what I'm working on and what I know about* (structured knowledge scoping).

---

## The Problem

A personal agent accumulates knowledge across many domains — work projects, hobbies, home infrastructure, family coordination, learning topics. Without structure, this knowledge either sits in a monolithic MEMORY.md that grows unwieldy, or fragments across daily logs where it's hard to locate.

The agent also has no concept of "I'm currently working on X" as a first-class construct. It knows what you said recently (via OM), but it doesn't have a persistent, organized understanding of your active work and reference material.

Users shouldn't have to manually switch contexts. The agent should recognize from conversation that a topic is relevant, activate the appropriate project context, and deactivate what's not needed — the same way you'd pull a project folder off a shelf when it's time to work on it.

---

## Entry Structure

Every project entry is a folder with a consistent internal layout. The entry is self-describing — there's no central registry. The agent discovers available contexts by scanning the `projects/` directory tree.

### context.yml

Each entry's root contains a `context.yml` that defines what the entry is and what capabilities it carries.

**Active project example:**

```yaml
name: aerohive-setup
description: "AeroHive AP network configuration using Ansible"
status: active
created: 2026-02-10
tools:
  - exec
  - read
  - write
skills:
  - ansible-playbooks
mcp_servers:
  - name: filesystem
    command: "mcp-server-filesystem"
    args: ["/home/user/ansible/aerohive"]
```

**Archived entry example:**

```yaml
name: proxmox-migration
description: "Migration from Proxmox to Docker-based homelab"
status: archived
created: 2025-11-01
archived: 2026-02-08
```

#### context.yml fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | yes | Human-readable identifier |
| `description` | string | yes | Brief summary of what this entry covers |
| `status` | string | yes | `active` or `archived` |
| `created` | date | yes | When the entry was created |
| `tools` | list | no | Tools to load when this context activates |
| `skills` | list | no | Skills to load when this context activates |
| `mcp_servers` | list | no | MCP servers to spin up when this context activates |
| `archived` | date | no | When the entry was archived (archive entries only) |

### Subfolder conventions

#### notes/

Agent-maintained memories, decisions, and observations specific to this context. This is the agent's internal knowledge about the entry — what it's learned, what decisions were made, what the current state looks like.

The agent writes here freely. This is the scoped equivalent of daily memory logs, but tied to a specific project.

Examples: `notes/decisions.md`, `notes/current-state.md`, `notes/blockers.md`

#### references/

User-added external material. Configs, documentation, PDFs, images, code snippets, links — anything the user wants the agent to have access to within this context.

The agent reads from here but treats it as source material, not something it modifies. Users can add, remove, or update files at any time.

Examples: `references/topology.png`, `references/ap01.conf`, `references/job-posting.pdf`

#### workspace/

Active working directory for the project. This is where the agent produces output — code, drafts, generated configs, build artifacts. It's the equivalent of a project's working tree.

Examples: `workspace/playbooks/`, `workspace/site/`, `workspace/draft-v2.md`

### Full layout

```
~/.ironclaw/workspace/
├── SOUL.md
├── USER.md
├── AGENTS.md
├── MEMORY.md
├── HEARTBEAT.yml
├── Alerts.md
├── memory/
│   ├── observations.json
│   ├── episodes/
│   └── YYYY-MM-DD.md
├── projects/
│   ├── aerohive-setup/
│   │   ├── context.yml
│   │   ├── notes/
│   │   │   ├── decisions.md
│   │   │   └── current-state.md
│   │   ├── references/
│   │   │   ├── topology.png
│   │   │   └── config/
│   │   │       └── ap01.conf
│   │   └── workspace/
│   │       └── playbooks/
│   │           └── configure-aps.yml
│   └── digi-application/
│       ├── context.yml
│       ├── notes/
│       │   └── interview-prep.md
│       ├── references/
│       │   └── job-posting.pdf
│       └── workspace/
│           └── resume-tailored.md
├── archive/
│   └── proxmox-migration/
│       ├── context.yml
│       ├── notes/
│       │   ├── decisions.md
│       │   └── final-state.md
│       ├── references/
│       └── workspace/
├── skills/
│   └── my-skill/
│       ├── SKILL.md
│       ├── scripts/
│       └── references/
└── cron/
    └── jobs.json
```

---

## Discovery & Activation

### Discovery

There is no central registry. The agent discovers available contexts by scanning the workspace:

1. Walk the `projects/` directory.
2. For each subfolder, read `context.yml`.
3. Build an in-memory index of available entries with their names, descriptions, statuses, and capability lists.

This scan happens at gateway startup and on filesystem changes (consistent with existing config hot-reload behavior).

The lightweight nature of context.yml (a few lines of YAML each) means scanning is cheap. The agent always knows what projects exist without loading their full contents.

### Activation

**Single-project constraint:** Only one project may be active at a time. Activating a new project automatically deactivates the current one.

The agent autonomously decides which project to activate based on the current conversation.

**Active context** means:
- The entry's `context.yml` metadata is available.
- Files from `notes/` are accessible (agent's knowledge about this project).
- Files from `references/` are accessible (user-provided material).
- Files from `workspace/` are accessible.
- Any `tools`, `skills`, and `mcp_servers` specified in context.yml are loaded.

**Inactive** means the agent knows the entry exists (name and description from the scan) but none of its contents or capabilities are loaded.

### Activation signals

The agent uses conversational cues to decide activation:

- User mentions a project by name or describes related work → activate
- Conversation shifts away from a topic → deactivate the current project

### Deactivation

The active project deactivates when the agent determines it's no longer relevant to the current conversation. Files are simply not included in the next context assembly, and tools/skills/MCP servers exclusive to that context are unloaded. Nothing is deleted or modified.

### Tool auto-loading

Projects can specify `tools`, `skills`, and `mcp_servers` in their context.yml. When the entry activates, those capabilities become available. When it deactivates, tools not needed by any other active context are unloaded.

**Tool resolution**: When a project is active, its tools are available. Deactivating a project removes tools that no other active context still needs.

**Security note**: Tool auto-loading follows the same trust model as the rest of the workspace — the user controls context.yml and can restrict which entries carry tool permissions. The agent can suggest adding tools to an entry, but the file is user-editable.

---

## Lifecycle Management

### Project completion

When the agent determines a project is complete — either through explicit user confirmation or by recognizing that the deliverable has been achieved — it automatically:

1. Updates the project's notes with a final state summary.
2. Sets `status: archived` and adds an `archived` date to context.yml.
3. Moves the project folder from `projects/` to `archive/`.

The agent does not require user permission, but should mention it: "I've archived the AeroHive setup project since the configuration is complete. You can still search its contents anytime."

### Creating new entries

The agent can create new project entries when it recognizes new work emerging from conversation:

- "I'm starting to work on setting up game servers for friends" → agent creates a new project with context.yml, notes/, references/, workspace/, and appropriate tools
- Repeated conversations about a topic with no existing entry → agent suggests creating one

Creation is self-contained: make the folder, write context.yml, create subfolders. No registry to update.

---

## Archive & Search

Archived entries are never loaded into active context. They're accessed exclusively through the existing hybrid search system (BM25 + vector). When the agent searches memory and gets hits from archived projects, it can reference that information in its response.

Archived content is indexed by the memory search system. The `archive/` directory is included in the memory search paths so that notes, references, and workspace contents from completed work remain searchable.

---

## Interaction with Other Systems

### Observational Memory

OM captures *what happened* chronologically. The Projects system captures *what the user is working on* structurally. They're complementary:

- OM episodes carry a `context` field tagging which project was active when they were generated (e.g., `"context": "aerohive-setup"`). This provides the linkage between the event timeline and structured project knowledge — episodes are searchable by project context without routing observations to separate per-project logs.
- When a project activates, the agent has both the structural knowledge (from the entry's files) and the recent history (from OM, filterable by context tag) available.

### Proactivity (Heartbeat/Pulses)

Pulse tasks can reference projects. A work-hours pulse might check the status of the active project. A daily review pulse might scan for stalled projects that haven't been touched recently.

### Identity layer

The Projects system is not a replacement for USER.md or MEMORY.md. Those files capture stable, cross-cutting information about the user. Projects capture scoped, domain-specific knowledge. A preference like "I like concise responses" belongs in USER.md. A note like "the AeroHive APs use firmware 8.2r4" belongs in the project's notes.

---

## Implementation Notes

### Priorities

1. Folder structure conventions and context.yml schema
2. Directory scanning and discovery at startup / on file change
3. Agent instructions for activation, deactivation, and notes maintenance
4. Tool, skill, and MCP server auto-loading
5. Automatic archiving on project completion
6. Archive indexing for search
7. Dynamic entry creation from conversation

### Considerations

- context.yml should be validated on scan with clear errors for malformed entries
- File watching should detect user-added files in references/ and make them available without restart
- Token budget management for active project contexts needs thought — notes/ should always be loadable when active, references/ and workspace/ may need selective loading for large entries
- The agent needs clear system prompt instructions for Projects behavior: when to activate, when to create, when to archive, how to maintain notes
- Creating a new entry should be as simple as `mkdir` + write context.yml + create subfolders — no external dependencies
- Archived entries should retain their full folder structure so search results have full context
