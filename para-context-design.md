# Personal AI Agent — PARA Context Management

## Overview

This document describes a context management system based on the PARA method (Projects, Areas, Resources, Archive) that gives the agent structured, scoped knowledge it can activate and deactivate autonomously. Each PARA entry is a self-contained, self-describing context folder with its own configuration, agent notes, reference material, and (for Projects) a working directory.

This system is independent of but complementary to the Memory & Proactivity design. OM handles *what happened* (episodic continuity). PARA handles *what I'm working on and what I know about* (structured knowledge scoping).

---

## The Problem

A personal agent accumulates knowledge across many domains — work projects, hobbies, home infrastructure, family coordination, learning topics. Without structure, this knowledge either sits in a monolithic MEMORY.md that grows unwieldy, or fragments across daily logs where it's hard to locate.

The agent also has no concept of "I'm currently working on X" as a first-class construct. It knows what you said recently (via OM), but it doesn't have a persistent, organized understanding of your active projects, ongoing responsibilities, or reference material.

Users shouldn't have to manually switch contexts. The agent should recognize from conversation that a topic is relevant, activate the appropriate context, and deactivate what's not needed — the same way you'd pull a project folder off a shelf when it's time to work on it.

---

## PARA Categories

### Projects

Active, time-bound work with a clear outcome or deliverable.

Examples: AeroHive network setup, Digi International job application, family dashboard app, resume site redesign.

**Lifecycle**: Created → Active → Completed → Archived automatically.

**Has**: context.yml, notes/, references/, workspace/

### Areas

Ongoing responsibilities with no end date. These represent domains the user continuously maintains.

Examples: homelab infrastructure, Overwatch coaching, family coordination, personal health.

**Lifecycle**: Created → Active (indefinitely) → Archived only if explicitly abandoned.

**Has**: context.yml, notes/, references/

### Resources

Reference material for topics of interest. Not tied to active work, but available when the topic comes up.

Examples: Ansible best practices, Kubernetes networking notes, poetry collection notes, TTRPG campaign world-building.

**Lifecycle**: Created → Available → Archived when no longer relevant.

**Has**: context.yml, notes/, references/

### Archive

Inactive items from all three categories above. Not loadable into active context, but fully searchable via hybrid retrieval. The archive is where completed projects, abandoned areas, and stale resources live.

---

## Entry Structure

Every PARA entry is a folder with a consistent internal layout. The entry is self-describing — there's no central registry. The agent discovers available contexts by scanning the `para/` directory tree.

### context.yml

Each entry's root contains a `context.yml` that defines what the entry is and what capabilities it carries.

**Project example:**

```yaml
name: aerohive-setup
description: "AeroHive AP network configuration using Ansible"
status: active
created: 2026-02-10
tools:
  - exec
  - read
  - write
```

**Area example:**

```yaml
name: homelab
description: "Home server infrastructure, Docker, monitoring, networking"
status: active
created: 2026-01-15
tools:
  - exec
  - read
  - write
  - browser
```

**Resource example:**

```yaml
name: ansible-patterns
description: "Ansible best practices, playbook patterns, role design"
status: available
created: 2026-01-20
```

**Archived entry example:**

```yaml
name: proxmox-migration
description: "Migration from Proxmox to Docker-based homelab"
status: archived
created: 2025-11-01
archived: 2026-02-08
original_type: project
```

#### context.yml fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | yes | Human-readable identifier |
| `description` | string | yes | Brief summary of what this entry covers |
| `status` | string | yes | `active`, `available`, or `archived` |
| `created` | date | yes | When the entry was created |
| `tools` | list | no | Tools to load when this context activates (Projects and Areas only) |
| `archived` | date | no | When the entry was archived (archive entries only) |
| `original_type` | string | no | `project`, `area`, or `resource` (archive entries only) |

### Subfolder conventions

#### notes/

Agent-maintained memories, decisions, and observations specific to this context. This is the agent's internal knowledge about the entry — what it's learned, what decisions were made, what the current state looks like.

The agent writes here freely. This is the PARA equivalent of daily memory logs, but scoped to a specific domain.

Examples: `notes/decisions.md`, `notes/current-state.md`, `notes/blockers.md`

#### references/

User-added external material. Configs, documentation, PDFs, images, code snippets, links — anything the user wants the agent to have access to within this context.

The agent reads from here but treats it as source material, not something it modifies. Users can add, remove, or update files at any time.

Examples: `references/topology.png`, `references/ap01.conf`, `references/job-posting.pdf`

#### workspace/ (Projects only)

Active working directory for the project. This is where the agent produces output — code, drafts, generated configs, build artifacts. It's the equivalent of a project's working tree.

Only Projects have a workspace/ folder. Areas and Resources don't produce deliverables in the same way — Areas are ongoing (notes and references suffice) and Resources are purely reference material.

Examples: `workspace/playbooks/`, `workspace/site/`, `workspace/draft-v2.md`

### Full layout

```
~/.openclaw/workspace/
├── SOUL.md
├── USER.md
├── AGENTS.md
├── MEMORY.md
├── HEARTBEAT.yml
├── Alerts.md
├── para/
│   ├── projects/
│   │   ├── aerohive-setup/
│   │   │   ├── context.yml
│   │   │   ├── notes/
│   │   │   │   ├── decisions.md
│   │   │   │   └── current-state.md
│   │   │   ├── references/
│   │   │   │   ├── topology.png
│   │   │   │   └── config/
│   │   │   │       └── ap01.conf
│   │   │   └── workspace/
│   │   │       └── playbooks/
│   │   │           └── configure-aps.yml
│   │   └── digi-application/
│   │       ├── context.yml
│   │       ├── notes/
│   │       │   └── interview-prep.md
│   │       ├── references/
│   │       │   └── job-posting.pdf
│   │       └── workspace/
│   │           └── resume-tailored.md
│   ├── areas/
│   │   ├── homelab/
│   │   │   ├── context.yml
│   │   │   ├── notes/
│   │   │   │   ├── inventory.md
│   │   │   │   └── maintenance-log.md
│   │   │   └── references/
│   │   │       └── network-diagram.png
│   │   └── overwatch-coaching/
│   │       ├── context.yml
│   │       ├── notes/
│   │       │   └── student-notes.md
│   │       └── references/
│   │           └── tier-list.md
│   ├── resources/
│   │   ├── ansible-patterns/
│   │   │   ├── context.yml
│   │   │   ├── notes/
│   │   │   │   └── key-takeaways.md
│   │   │   └── references/
│   │   │       └── playbook-examples.md
│   │   └── k8s-networking/
│   │       ├── context.yml
│   │       ├── notes/
│   │       │   └── cni-comparison.md
│   │       └── references/
│   └── archive/
│       └── proxmox-migration/
│           ├── context.yml
│           ├── notes/
│           │   ├── decisions.md
│           │   └── final-state.md
│           ├── references/
│           └── workspace/
```

---

## Discovery & Activation

### Discovery

There is no central registry. The agent discovers available contexts by scanning the `para/` directory:

1. Walk `para/projects/`, `para/areas/`, `para/resources/` directories.
2. For each subfolder, read `context.yml`.
3. Build an in-memory index of available entries with their names, descriptions, statuses, and tool lists.

This scan happens at gateway startup and on filesystem changes (consistent with existing config hot-reload behavior).

The lightweight nature of context.yml (a few lines of YAML each) means scanning is cheap. The agent always knows what contexts exist without loading their full contents.

### Activation

The agent autonomously decides which PARA entries to load into context based on the current conversation.

**Active context** means:
- The entry's `context.yml` metadata is available.
- Files from `notes/` are loaded (agent's knowledge about this domain).
- Files from `references/` are accessible (user-provided material).
- For Projects, `workspace/` contents are accessible.
- Any `tools` specified in context.yml are made available.

**Inactive** means the agent knows the entry exists (name and description from the scan) but none of its contents or tools are loaded.

### Activation signals

The agent uses conversational cues to decide activation:

- User mentions a context by name or describes related work → activate
- Conversation touches an area of ongoing responsibility → activate that area
- A resource topic becomes relevant to the current work → activate that resource
- Conversation shifts away from a topic → deactivate entries no longer relevant

### Multiple simultaneous contexts

There are no limits on how many entries can be active at once. In practice, a conversation about the AeroHive setup might activate:

- `aerohive-setup` (the project itself)
- `homelab` (the area it falls under)
- `ansible-patterns` (a relevant resource)

The agent manages the total token budget. If active contexts consume too much of the context window, the agent can selectively load only key files from lower-priority entries rather than full folder contents.

### Deactivation

Contexts deactivate when the agent determines they're no longer relevant to the current conversation. Files are simply not included in the next context assembly, and any tools exclusive to that context are unloaded. Nothing is deleted or modified.

### Tool auto-loading

Projects and Areas can specify a `tools` list in their context.yml. When the entry activates, those tools become available. When it deactivates, tools not needed by any other active context are unloaded.

```yaml
# When aerohive-setup activates, the agent gets:
# - The project's notes and references (knowledge)
# - The project's workspace (working directory)
# - exec, read, write tools (capabilities to run Ansible)
```

Tool lists are optional. An entry with no `tools` field activates only its knowledge context. Resources never specify tools — they're reference material, not active workspaces.

**Tool resolution**: When multiple active entries specify tools, the union of all tool lists is available. If any active context grants a tool, it's available. Deactivating a context only removes tools that no other active context still needs.

**Security note**: Tool auto-loading follows the same trust model as the rest of the workspace — the user controls context.yml and can restrict which entries carry tool permissions. The agent can suggest adding tools to an entry, but the file is user-editable.

---

## Lifecycle Management

### Project completion

When the agent determines a project is complete — either through explicit user confirmation or by recognizing that the deliverable has been achieved — it automatically:

1. Updates the project's notes with a final state summary.
2. Sets `status: archived`, adds `archived` date and `original_type: project` to context.yml.
3. Moves the project folder from `para/projects/` to `para/archive/`.

The agent does not require user permission, but should mention it: "I've archived the AeroHive setup project since the configuration is complete. You can still search its contents anytime."

### Creating new entries

The agent can create new PARA entries when it recognizes a new project, area, or resource emerging from conversation:

- "I'm starting to work on setting up game servers for friends" → agent creates a new project with context.yml, notes/, references/, workspace/, and appropriate tools
- Repeated conversations about a topic with no existing entry → agent suggests creating one

Creation is self-contained: make the folder, write context.yml, create subfolders. No registry to update.

### Promoting between categories

Entries can move between categories as their nature changes:

- A Resource that becomes active work → promote to Project (add workspace/, move to projects/)
- A Project that becomes an ongoing responsibility → promote to Area (drop workspace/, move to areas/)
- An Area that becomes dormant → archive

When promoting a Resource to a Project, the agent creates the `workspace/` subfolder and moves the folder from `para/resources/` to `para/projects/`.

---

## Archive & Search

Archived entries are never loaded into active context. They're accessed exclusively through the existing hybrid search system (BM25 + vector). When the agent searches memory and gets hits from archived PARA entries, it can reference that information in its response.

Archived content should be indexed by the memory search system. The `para/archive/` directory should be included in the memory search paths so that notes, references, and workspace contents from completed work remain searchable.

---

## Interaction with Other Systems

### Observational Memory

OM captures *what happened* chronologically. PARA captures *what the user is working on* structurally. They're complementary:

- OM episodes carry a `context` field tagging which PARA entry was active when they were generated (e.g., `"context": "aerohive-setup"`). This provides the linkage between the event timeline and structured project knowledge — episodes are searchable by PARA context without routing observations to separate per-project logs.
- When a PARA context activates, the agent has both the structural knowledge (from the entry's files) and the recent history (from OM, filterable by context tag) available.

### Proactivity (Heartbeat/Pulses)

Pulse tasks can reference PARA entries. A work-hours pulse might check the status of active projects. A daily review pulse might scan for stalled projects or areas that haven't been touched recently.

The agent could also generate pulse tasks dynamically based on active projects — "the AeroHive setup has a deadline next Friday, add a daily check for it." This emerges from the agent's discretion, not from hardcoded behavior.

### Identity layer

PARA is not a replacement for USER.md or MEMORY.md. Those files capture stable, cross-cutting information about the user. PARA captures scoped, domain-specific knowledge. A preference like "I like concise responses" belongs in USER.md. A note like "the AeroHive APs use firmware 8.2r4" belongs in the project's notes.

---

## Implementation Notes

### Priorities

1. Folder structure conventions and context.yml schema
2. Directory scanning and discovery at startup / on file change
3. Agent instructions for activation, deactivation, and notes maintenance
4. Tool auto-loading and union-based resolution
5. Automatic archiving on project completion
6. Archive indexing for search
7. Dynamic entry creation from conversation
8. Category promotion (resource → project, project → area)

### Considerations

- context.yml should be validated on scan with clear errors for malformed entries
- File watching should detect user-added files in references/ and make them available without restart
- Token budget management for multiple active contexts needs thought — notes/ should always be loadable when active, references/ and workspace/ may need selective loading for large entries
- The agent needs clear system prompt instructions for PARA behavior: when to activate, when to create, when to archive, how to maintain notes
- Creating a new entry should be as simple as `mkdir` + write context.yml + create subfolders — no external dependencies
- Archived entries should retain their full folder structure so search results have full context
