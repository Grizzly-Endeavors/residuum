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

### PROJECT.md

Each entry's root contains a `PROJECT.md` that defines what the entry is, what capabilities it carries, and any overview content the user wants the agent to know when entering this context. It uses YAML frontmatter for structured metadata and a markdown body for the human-maintained overview — the same format as `SKILL.md`.

**Active project example:**

```markdown
---
name: aerohive-setup
description: "AeroHive AP network configuration using Ansible"
status: active
created: 2026-02-10
tools:
  - exec
  - read
  - write
mcp_servers:
  - name: filesystem
    command: "mcp-server-filesystem"
    args: ["/home/user/ansible/aerohive"]
---

Configuring 12 APs across the office using Ansible. APs use HiveManager CLI,
not aoscli. Per-AP configuration lives in host_vars/ files. Primary contact
for physical access is facilities@example.com.
```

**Archived entry example:**

```markdown
---
name: proxmox-migration
description: "Migration from Proxmox to Docker-based homelab"
status: archived
created: 2025-11-01
archived: 2026-02-08
---
```

#### Frontmatter fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | yes | Human-readable identifier |
| `description` | string | yes | Brief summary of what this entry covers |
| `status` | string | yes | `active` or `archived` |
| `created` | date | yes | When the entry was created |
| `tools` | list | no | Tools to load when this context activates |
| `mcp_servers` | list | no | MCP servers to spin up when this context activates |
| `archived` | date | no | When the entry was archived (archive entries only) |

The body of `PROJECT.md` is an overview the agent maintains and reads automatically when the project activates — purpose, current state, key constraints, anything worth surfacing on entry. The agent updates it as the project evolves. Users can edit it directly if they want to guide or correct the agent's understanding.

### Subfolder conventions

#### notes/

Agent-maintained memories, decisions, and observations specific to this context. This is the agent's internal knowledge about the entry — what it's learned, what decisions were made, what the current state looks like.

The agent writes here freely. This is the scoped equivalent of daily memory logs, but tied to a specific project. Users can read and edit these files to guide or correct the agent's understanding.

Examples: `notes/decisions.md`, `notes/current-state.md`, `notes/blockers.md`

`notes/log.md` is reserved for the `project_log` tool (see below) and should not be written to directly.

#### references/

External material relevant to the project — configs, documentation, PDFs, images, code snippets, links. Both the agent and the user can add files here. Users commonly drop in source material (job postings, topology diagrams, vendor configs) that the agent can't fetch itself. The agent can also save reference material it retrieves or generates.

Examples: `references/topology.png`, `references/ap01.conf`, `references/job-posting.pdf`

#### workspace/

Active working directory for the project. This is where the agent produces output — code, drafts, generated configs, build artifacts. It's the equivalent of a project's working tree.

Examples: `workspace/playbooks/`, `workspace/site/`, `workspace/draft-v2.md`

#### skills/

Project-scoped skills that are only relevant when this project is active. Each skill is a subfolder containing a `SKILL.md` following the Agent Skills spec. When the project activates, the loader discovers skills here and adds their metadata to the available set. When the project deactivates, these skills are removed.

This keeps project-specific skills colocated with the project they belong to — no need to pollute global skill directories with skills only one project uses. The project folder remains fully self-contained and portable.

Global skills (in `~/.residuum/skills/` or `~/.residuum/workspace/skills/`) are always visible regardless of which project is active. The `skills/` subfolder is for skills that only make sense in this project's context.

Examples: `skills/ansible-playbooks/SKILL.md`, `skills/ap-diagnostics/SKILL.md`

### Directory name sanitization

Directory names are derived from the project name: lowercased, spaces and special characters replaced with hyphens, multiple hyphens collapsed, leading/trailing hyphens trimmed. For example, `"AeroHive Setup v2.0!"` becomes `aerohive-setup-v2-0`.

### Full layout

```
~/.residuum/workspace/
├── SOUL.md
├── USER.md
├── AGENTS.md
├── MEMORY.md
├── ENVIRONMENT.md
├── PRESENCE.toml
├── HEARTBEAT.yml
├── CHANNELS.yml
├── scheduled_actions.json
├── memory/
│   ├── observations.json
│   └── episodes/
│       └── YYYY-MM/DD/
├── inbox/
├── subagents/
├── projects/
│   ├── aerohive-setup/
│   │   ├── PROJECT.md          ← frontmatter + overview body
│   │   ├── notes/
│   │   │   ├── log/            ← written by project_deactivate (required)
│   │   │   │   └── YYYY-MM/
│   │   │   │       └── log-DD.md
│   │   │   ├── decisions.md
│   │   │   └── current-state.md
│   │   ├── references/
│   │   │   ├── topology.png
│   │   │   └── config/
│   │   │       └── ap01.conf
│   │   ├── skills/
│   │   │   └── ansible-playbooks/
│   │   │       ├── SKILL.md
│   │   │       └── references/
│   │   └── workspace/
│   │       └── playbooks/
│   │           └── configure-aps.yml
│   └── digi-application/
│       ├── PROJECT.md
│       ├── notes/
│       │   └── interview-prep.md
│       ├── references/
│       │   └── job-posting.pdf
│       └── workspace/
│           └── resume-tailored.md
├── archive/
│   └── proxmox-migration/
│       ├── PROJECT.md
│       ├── notes/
│       │   ├── decisions.md
│       │   └── final-state.md
│       ├── references/
│       └── workspace/
└── skills/
    └── my-skill/
        ├── SKILL.md
        ├── scripts/
        └── references/
```

---

## Discovery & Activation

### Discovery

There is no central registry. The agent discovers available contexts by scanning the workspace:

1. Walk the `projects/` directory.
2. For each subfolder, read `PROJECT.md` and parse the YAML frontmatter.
3. Build an in-memory index of available entries with their names, descriptions, statuses, and capability lists.

This scan happens at gateway startup and on filesystem changes (consistent with existing config hot-reload behavior).

The frontmatter is a few lines of YAML each, so scanning is cheap. The body of `PROJECT.md` is not loaded during scanning — only the frontmatter. The agent always knows what projects exist without loading their full contents.

### Activation

**Single-project constraint:** Only one project may be active at a time. Activating a new project automatically deactivates the current one.

The agent autonomously decides which project to activate based on the current conversation.

**Activation steps** (in order):

1. `PROJECT.md` frontmatter and body are loaded into context.
2. A manifest is built by scanning subdirectories (`notes/`, `references/`, `workspace/`, `skills/`). The manifest lists what files exist — file contents are NOT loaded. The agent reads specific files on demand via `read_file`.
3. Recent session logs (~2000 tokens from the most recent `notes/log/` entries) are loaded to give the agent immediate continuity context.
4. The tool filter is updated to the project's listed tools (plus always-allowed tools like `read`, `project_deactivate`, etc.).
5. MCP servers declared in the frontmatter are started (with reference counting — see below).
6. The path policy is scoped to the project directory: writes are restricted to the active project's subtree. The agent can still read from any directory freely. Writes to `projects/` outside the active project, and all writes to `archive/`, are rejected.
7. Project-scoped skills (in the project's `skills/` subdirectory) are discovered and added to the available skill index.

**Inactive** means the agent knows the entry exists (name and description from the scan) but none of its contents or capabilities are loaded.

### Activation signals

The agent uses conversational cues to decide activation:

- User mentions a project by name or describes related work → activate
- Conversation shifts away from a topic → deactivate the current project

### Write scoping

When a project is active, the path policy restricts file writes to the active project's directory subtree. Reads are unrestricted — the agent can read from any directory in the workspace (or outside it) at any time. This scoping prevents accidental writes to other projects or archived content.

Workspace-level files (`MEMORY.md`, `memory/`, `scheduled_actions.json`, etc.) remain writable regardless of which project is active. Only the `projects/` and `archive/` directories are subject to write restrictions.

### Deactivation

The active project deactivates when the agent determines it's no longer relevant to the current conversation. Files are simply not included in the next context assembly, and tools/MCP servers/project-scoped skills exclusive to that context are unloaded. Nothing is deleted or modified.

**Gateway-enforced requirement:** `project_deactivate` requires a `log` field containing a non-empty session summary. The gateway writes the entry to the project's dated log file and then performs the deactivation. A call without a `log` value is rejected. This is enforced by the gateway at the tool call level — it is not a prompt instruction the agent can reason around.

### project_log

Entries are written to `notes/log/YYYY-MM/log-DD.md` within the active project's folder — one file per day, accumulated across sessions. Each entry has a `**HH:MM**` timestamp prefix, with the date as a section header.

```markdown
# 2026-02-23

- **14:32** Reviewed ap01.conf — channel assignment conflicts with ap03 on 5GHz band. Updated notes/current-state.md with findings.
- **14:45** Generated configure-aps.yml playbook targeting host_vars layout. Needs testing against staging AP.
```

Rather than a separate `project_log` tool, logging is integrated directly into `project_deactivate` as a required `log` field. The gateway writes the log entry and then performs the deactivation atomically. A `project_deactivate` call with a missing or empty `log` field is rejected. This keeps the session record requirement without adding an extra tool call round-trip.

The `notes/log/` subtree is reserved for this purpose and should not be written to via the `write` tool directly.

### Tool auto-loading

Projects can specify `tools` and `mcp_servers` in their `PROJECT.md` frontmatter, and include project-scoped skills in a `skills/` subdirectory. When the entry activates, those capabilities become available. When it deactivates, tools and skills not needed by any other active context are unloaded.

**Tool resolution**: When a project is active, its tools are available. Deactivating a project removes tools that no other active context still needs.

**MCP server reference counting**: MCP servers defined in `PROJECT.md` use reference counting. Multiple agents (main agent + sub-agents, or multiple sub-agents) can have the same project active simultaneously without premature teardown. A server starts on the first activation (count 0 to 1) and tears down only when the last deactivation drops the count to zero.

**Background task force-deactivation**: If a sub-agent's turn loop ends with a project still active, the gateway force-deactivates the project with an auto-generated log entry: `"[auto] SubAgent {id} completed without deactivating."` This ensures every sub-agent interaction with a project gets logged, maintaining session log continuity.

**Security note**: Tool auto-loading follows the same trust model as the rest of the workspace — the agent manages `PROJECT.md` but the user can edit it directly to restrict or adjust which tool permissions an entry carries.

### Project tools

Five tools manage the project lifecycle:

**`project_create`** — Create a new project with the standard directory structure and `PROJECT.md`.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `name` | string | yes | Human-readable project name |
| `description` | string | yes | Brief summary of what this project covers |
| `tools` | array of strings | no | Tool names to associate with this project |

**`project_activate`** — Activate a project context. Loads the project's overview, manifest, and configuration.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `name` | string | yes | Name of the project to activate (case-insensitive) |

**`project_deactivate`** — Deactivate the current project context. Requires a non-empty session summary.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `log` | string | yes | Session summary log entry (must not be empty) |

**`project_archive`** — Archive a completed project. Updates frontmatter to archived status and moves to `archive/`.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `name` | string | yes | Name of the project to archive |

**`project_list`** — List all projects and their status. No required parameters.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `include_archived` | boolean | no | Include archived projects in the list (default false) |

---

## Lifecycle Management

### Project completion

When the agent determines a project is complete — either through explicit user confirmation or by recognizing that the deliverable has been achieved — it automatically:

1. Updates the project's notes with a final state summary.
2. Sets `status: archived` and adds an `archived` date to the `PROJECT.md` frontmatter.
3. Moves the project folder from `projects/` to `archive/`.

The agent does not require user permission, but should mention it: "I've archived the AeroHive setup project since the configuration is complete. You can still search its contents anytime."

### Creating new entries

The agent can create new project entries when it recognizes new work emerging from conversation:

- "I'm starting to work on setting up game servers for friends" → agent creates a new project with `PROJECT.md`, `notes/`, `references/`, `workspace/`, and appropriate tools
- Repeated conversations about a topic with no existing entry → agent suggests creating one

Creation is self-contained: make the folder, write `PROJECT.md`, create subfolders. No registry to update.

---

## Archive & Search

Archived projects are moved to `archive/` and are not loaded into active context by default. They can be reactivated — the scanner indexes both `projects/` and `archive/`, so archived entries appear in `project_list` (with an `archived` status). Reactivation would move the project back to `projects/` and update its status. Archived content is also searchable through the memory search system (BM25).

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

1. Folder structure conventions and `PROJECT.md` frontmatter schema
2. Directory scanning and discovery at startup / on file change
3. Agent instructions for activation, deactivation, and notes maintenance
4. Tool, MCP server, and project-scoped skill auto-loading
5. Automatic archiving on project completion
6. Archive indexing for search
7. Dynamic entry creation from conversation

### Considerations

- `PROJECT.md` frontmatter should be validated on scan with clear errors for malformed entries
- The `PROJECT.md` body is always loaded when a project is active — keep it concise; it occupies prime token budget
- File watching should detect new files in references/ (from either agent or user) and make them available without restart
- Token budget management for active project contexts needs thought — notes/ should always be loadable when active, references/ and workspace/ may need selective loading for large entries
- The agent needs clear system prompt instructions for Projects behavior: when to activate, when to create, when to archive, how to maintain notes
- Creating a new entry should be as simple as `mkdir` + write context.yml + create subfolders — no external dependencies
- Archived entries should retain their full folder structure so search results have full context
