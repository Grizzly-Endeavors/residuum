---
name: wiki
description: Page format, index rules, and ingest/lint procedures for the knowledge wiki in wiki/. Activate before creating, editing, moving, or deleting any wiki page. Also the role of the memory_tending and wiki_lint pulses.
---

# Knowledge Wiki

`wiki/` is your long-term knowledge: an Open Knowledge Format (OKF) bundle of Markdown pages. Its root `index.md` is in your prompt as WIKI_INDEX; everything else you read on demand with read_file.

## Pages

One concept per page: a person, a project, a tool, a place, a preference area, a machine. File names are lowercase-kebab-case (`home-network.md`).

Every page starts with YAML frontmatter:

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
stale_after: 2027-03-01T00:00:00Z
---

# Grizzly Platform

Body in plain Markdown. Link related pages: [homelab cluster](/homelab/cluster.md).
```

- `type` (required): a short noun phrase. Reuse types already in the wiki before inventing one. Common ones: `Person`, `Project`, `Tool`, `Machine`, `Preference`, `Topic`, `Place`, `Organization`.
- `title` and `description` (always set): the description is the one line that goes into the index, so make it say what the page tells you.
- `status`: `draft` for something seen once and not yet corroborated, `stable` once it is confirmed, `deprecated` when it no longer holds but is worth keeping for history. Default `stable`.
- `sources`: where the knowledge came from. For conversation evidence use `resource: episode:<episode id>` so memory_get can pull the transcript. Add an entry each time new evidence confirms or changes the page.
- `stale_after`: set it when a fact has a natural shelf life (versions, current jobs, ongoing projects). Omit it for durable facts.
- `tags`: optional, for grouping across folders.

Write the body as declarative facts ("The user prefers X"), not instructions to yourself. Keep one page per concept: before creating a page, search the index (and memory_search) for an existing one and update it instead.

## Links

Link other pages with bundle-relative paths starting with `/`, where `/` is `wiki/`: `[cluster](/homelab/cluster.md)`. Link whenever a page mentions a concept that has its own page.

## Folders and indexes

Start pages at the wiki root. When a group of related pages crowds the root index (roughly 8 or more on one theme), move them into a folder and give that folder its own `index.md`. Nest again only when a folder's own index gets crowded the same way.

Every directory has an `index.md`. Index files have no frontmatter (the root one keeps its `okf_version` line). Each entry is one line:

```markdown
- [Grizzly Platform](/grizzly-platform.md) — The user's self-hosted infrastructure repo.
- [Homelab](/homelab/index.md) — Cluster hardware, networking, and services (6 pages).
```

List every page in its own folder's index, and list every subfolder in its parent's index with a one-line summary. Group entries under `##` headings when an index passes about a dozen lines.

## After every change

1. Update the `index.md` of each folder you touched: add new pages, fix moved or renamed links, drop deleted ones, refresh descriptions that changed.
2. Append one line to `wiki/log.md`: `## [YYYY-MM-DD] <ingest|edit|restructure|lint> | <what changed and where>`.
3. Re-read each index you edited and confirm every link points at a file that exists.

## USER.md versus the wiki

USER.md holds only the user's core facts: a capped list (about 15) of durable identity and standing preferences needed on every turn. When it is full, replace an entry rather than adding one. Everything else about the user goes in wiki pages.

## Ingest (memory_tending pulse)

1. Use memory_search and memory_get to read the episodes and observations since the last `ingest` entry in `wiki/log.md`.
2. For each durable fact, decision, or preference: update the existing page that covers it, or create a new page. Add the episode to its `sources`.
3. A pattern seen once goes in a `draft` page. Promote a draft to `stable` once a second, independent episode supports it.
4. Correct pages the new evidence contradicts; mark pages `deprecated` when they stop being true.
5. Update USER.md only for core facts, under the cap.
6. Finish with the After-every-change steps.

## Lint (wiki_lint pulse)

Walk every folder and fix what you find:

- Pages missing from their folder's index, index links to files that do not exist, subfolders missing from the parent index.
- Pages missing `type`, `title`, or `description`, or whose description no longer matches the body.
- Pages past `stale_after`: re-verify from recent episodes and update or extend the date; mark `deprecated` if no longer true.
- `draft` pages older than a month with no new evidence: delete them.
- Two pages covering the same concept: merge them and fix the links.
- Contradictions between pages: resolve from the most recent evidence.
- Concepts mentioned on several pages that deserve their own page, and pages that should link each other but do not.
- Folders that have grown crowded (split) or nearly empty (fold back into the parent).

Log one `lint` entry summarizing the fixes.
