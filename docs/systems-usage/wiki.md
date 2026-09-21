# Knowledge Wiki

The wiki is where the agent keeps distilled long-term knowledge: facts about the user, the people and projects in their life, their tools, their machine, and their standing preferences. It lives in the workspace at `wiki/` and follows the [Open Knowledge Format](https://github.com/GoogleCloudPlatform/open-knowledge-format) (OKF), a formalization of the "LLM wiki" pattern: a folder of Markdown pages, one concept per page, with YAML frontmatter, cross-links, and index files an agent reads before drilling into pages.

The wiki is entirely agent-maintained. The user can read and edit it through the web UI's file editor like any other workspace file.

## Layout

```
wiki/
├── index.md          # Root catalog — injected into every prompt as WIKI_INDEX
├── log.md            # Append-only history of wiki changes
├── <concept>.md      # A page at the root
└── <folder>/
    ├── index.md      # Catalog of this folder
    └── <concept>.md
```

- `index.md` and `log.md` are reserved names at every level; every other `.md` file is a concept page.
- Pages start at the root. When a group of related pages crowds the root index (roughly eight or more on one theme), they move into a folder with its own `index.md`, and the root index lists the folder with a one-line summary. Folders nest the same way when they get crowded.
- Index entries are one line each: a bundle-relative link and the page's `description`.

## Pages

```markdown
---
type: Project
title: Grizzly Platform
description: The user's self-hosted infrastructure repo — Terraform, Ansible, and Flux-managed k8s.
tags: [infrastructure, homelab]
status: stable
sources:
  - resource: episode:ep-042
    title: Walked through the Flux layout
    last_modified: 2026-09-14
stale_after: 2027-03-01T00:00:00Z
---

# Grizzly Platform

Body in plain Markdown, linking related pages: [homelab cluster](/homelab/cluster.md).
```

| Field | Use |
|-------|-----|
| `type` | Required. Short noun phrase (`Person`, `Project`, `Tool`, `Machine`, `Preference`, …). |
| `title`, `description` | Always set. The description is what appears in the index. |
| `status` | `draft` (one episode supports it), `stable` (a second independent episode does), `deprecated` (no longer true, kept for history). Defaults to `stable`. |
| `sources` | One entry per supporting episode: `resource: episode:ep-NNN` (retrievable with `memory_get`) and the episode's date in `last_modified`. |
| `stale_after` | Set for facts with a shelf life; `wiki_lint` re-verifies pages past it. |
| `tags` | Optional cross-folder grouping. |

Links are bundle-relative, starting with `/` (where `/` is `wiki/`), so they survive pages moving between folders. Page bodies are declarative facts about the user and their world, never instructions to the agent.

## How it reaches the model

Only the root `index.md` is in the prompt, as the `WIKI_INDEX` section — for the main agent and every sub-agent. The agent reads the index, then opens the pages it needs with `read_file`, following folder indexes down as far as it needs. This keeps the prompt size bounded by the index rather than by everything the agent knows, and it is why each page's `description` and each index must stay accurate: the index is how the agent decides what to open.

The prompt is rebuilt from disk every turn, so index edits show up on the next turn without a restart.

Wiki pages are not indexed by `memory_search`; the indexes are how pages are found.

## Who writes it

- **The main agent**, during conversation, whenever it learns something durable. It activates the bundled `wiki` skill before writing, which carries the page format, index rules, and procedures.
- **`memory_tending`** (nightly, runs as the `wiki` skill): ingests every episode after the one recorded in the last `ingest` entry of `log.md` — episode IDs are sequential, so it walks them with `memory_get` rather than searching — and files what they teach into pages and `USER.md`. It reads at most 20 episodes per run and records the last one it read.
- **`wiki_lint`** (weekly, runs as the `wiki` skill): fixes index drift (pages missing from indexes, links to missing files, unlisted folders), incomplete frontmatter, pages past `stale_after`, drafts whose newest source is over a month old, duplicate pages, contradictions, and missing cross-links; it also splits crowded folders and folds near-empty ones back.
- **The `learner` skill**: files corroborated preference signals from the subconscious as pages.

Every change updates the index of each folder touched and appends a line to `log.md` (`## [YYYY-MM-DD] <ingest|edit|restructure|lint> | …`; ingest entries also record `through ep-NNN`).

Indexes are maintained by the agent rather than generated from frontmatter, so it can group and summarize them; the cost is that they can drift, which is what `wiki_lint` exists to catch.

## Relationship to other files

- **`USER.md`** stays outside the wiki and in the prompt: a capped list of core facts needed every turn. See [memory.md](memory.md#usermd--core-facts).
- **The memory pipeline** (episodes, observations) is the raw record; the wiki is the distilled layer built from it. `sources` link the two.
- **`SOUL.md`, `AGENTS.md`, and the `HARNESS` section** are instructions, not knowledge, and stay outside the wiki.
