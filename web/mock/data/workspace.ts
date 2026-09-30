import type { WorkspaceEntry } from "../../src/lib/types";
import type { MockClock } from "../env";
import { dirEntryVersion, fileVersion } from "../workspace-tree";

/** A directory entry as the sample tree declares it: its modification time and version are added when the tree is built. */
type SampleEntry = Pick<WorkspaceEntry, "name" | "entry_type" | "size">;

const SAMPLE_TREE: Record<string, SampleEntry[]> = {
  "": [
    { name: "SOUL.md", entry_type: "file", size: 847 },
    { name: "PRESENCE.toml", entry_type: "file", size: 245 },
    { name: "HEARTBEAT.yml", entry_type: "file", size: 178 },
    { name: "CHANNELS.yml", entry_type: "file", size: 392 },
    { name: "team", entry_type: "directory", size: null },
    { name: "memory", entry_type: "directory", size: null },
    { name: "skills", entry_type: "directory", size: null },
    { name: "config", entry_type: "directory", size: null },
    { name: "inbox", entry_type: "directory", size: null },
    { name: "subagents", entry_type: "directory", size: null },
    { name: "archive", entry_type: "directory", size: null },
  ],
  skills: [
    { name: "research", entry_type: "directory", size: null },
    { name: "code-review", entry_type: "directory", size: null },
  ],
  "skills/research": [
    { name: "SKILL.md", entry_type: "file", size: 634 },
    { name: "prompt.md", entry_type: "file", size: 1102 },
  ],
  "skills/code-review": [{ name: "SKILL.md", entry_type: "file", size: 478 }],
  config: [
    { name: "mcp.json", entry_type: "file", size: 1567 },
    { name: "channels.toml", entry_type: "file", size: 834 },
    { name: "agent-card.json", entry_type: "file", size: 356 },
  ],
  team: [
    { name: "AGENTS.md", entry_type: "file", size: 523 },
    { name: "USER.md", entry_type: "file", size: 312 },
    { name: "wiki", entry_type: "directory", size: null },
    { name: "workbench", entry_type: "directory", size: null },
  ],
  "team/workbench": [],
  "team/wiki": [
    { name: "index.md", entry_type: "file", size: 512 },
    { name: "log.md", entry_type: "file", size: 340 },
    { name: "projects", entry_type: "directory", size: null },
  ],
  "team/wiki/projects": [
    { name: "index.md", entry_type: "file", size: 210 },
    { name: "residuum.md", entry_type: "file", size: 486 },
  ],
  memory: [
    { name: "observations.jsonl", entry_type: "file", size: 45230 },
    { name: "reflections.jsonl", entry_type: "file", size: 12450 },
  ],
  inbox: [],
  subagents: [],
  archive: [],
};

/**
 * The sample workspace tree: directory path to its entries, each with the
 * size and version the listing reports. `team` is the shared team tree.
 * `contents` are the sample files, whose version a read of the file reports.
 */
export function createWorkspaceFiles(
  contents: Readonly<Record<string, string>>,
  clock: MockClock,
): Record<string, WorkspaceEntry[]> {
  const modified = clock.now();
  const tree: Record<string, WorkspaceEntry[]> = {};
  for (const [dir, entries] of Object.entries(SAMPLE_TREE)) {
    tree[dir] = entries.map((entry) => {
      const path = dir === "" ? entry.name : `${dir}/${entry.name}`;
      const version =
        entry.entry_type === "directory"
          ? dirEntryVersion(path, modified)
          : fileVersion(contents[path] ?? "");
      return { ...entry, modified, version };
    });
  }
  return tree;
}

/** The sample file contents, by workspace path. */
export function createWorkspaceFileContents(): Record<string, string> {
  return {
    "SOUL.md":
      "# Soul\n\nI am Residuum, a personal AI agent framework designed for long-running autonomous operation.\n\n## Core Identity\n\n- I maintain persistent memory across conversations\n- I operate with genuine agency, not just reactivity\n- I respect my operator's preferences and working style\n- I am transparent about my capabilities and limitations\n\n## Values\n\n- **Honesty**: I never fabricate information or hide errors\n- **Autonomy**: I take initiative when appropriate\n- **Memory**: I remember and build on past interactions\n- **Craft**: I strive for quality in everything I produce\n",
    "team/AGENTS.md":
      "# Agents\n\n## Active Agents\n\n### Observer\nMonitors context window usage and triggers memory extraction.\n- Threshold: 30,000 tokens\n- Frequency: Checked after each turn\n\n### Reflector\nSynthesizes observations into higher-level reflections.\n- Threshold: 40,000 tokens\n- Minimum observations: 5\n\n### Pulse\nRuns periodic system health checks.\n- Interval: 5 minutes\n- Reports: memory stats, token usage, active tasks\n",
    "team/USER.md":
      "# User Profile\n\n- **Name**: Bear\n- **Timezone**: America/New_York\n- **Preferred communication**: Direct and concise\n- **Working hours**: Flexible, mostly evenings\n",
    "PRESENCE.toml":
      '[presence]\nstatus = "active"\nlast_seen = "2026-03-10T14:30:00Z"\n\n[presence.channels]\nweb = true\ndiscord = false\ntelegram = true\n',
    "HEARTBEAT.yml":
      'interval_seconds: 300\nchecks:\n  - memory_usage\n  - token_count\n  - active_tasks\n  - channel_status\nlast_beat: "2026-03-10T14:30:00Z"\nstatus: healthy\n',
    "CHANNELS.yml":
      'channels:\n  web:\n    enabled: true\n    priority: high\n  discord:\n    enabled: false\n    token_ref: "secret:discord_token"\n  telegram:\n    enabled: true\n    token_ref: "secret:telegram_token"\n    chat_id: "123456789"\n',
    "skills/research/SKILL.md":
      '# Research Skill\n\n## Purpose\nConduct thorough research on topics using available tools and memory.\n\n## Triggers\n- User asks to "research" or "look into" a topic\n- User asks for comprehensive analysis\n\n## Process\n1. Search memory for existing knowledge\n2. Use web search if available\n3. Synthesize findings\n4. Store key observations\n',
    "skills/research/prompt.md":
      "You are conducting research on the following topic: {{topic}}\n\n## Guidelines\n- Search memory first for existing knowledge\n- Use web search tools if available\n- Cross-reference multiple sources\n- Note confidence levels for each finding\n- Store important observations for future reference\n\n## Output Format\n- Summary (2-3 sentences)\n- Key findings (bulleted list)\n- Sources and confidence levels\n- Suggested follow-up questions\n",
    "skills/code-review/SKILL.md":
      "# Code Review Skill\n\n## Purpose\nReview code changes for quality, correctness, and style.\n\n## Triggers\n- User asks for code review\n- PR review requests\n\n## Checklist\n- [ ] Logic correctness\n- [ ] Error handling\n- [ ] Style consistency\n- [ ] Test coverage\n- [ ] Security considerations\n",
    "config/mcp.json":
      '{\n  "servers": {\n    "filesystem": {\n      "command": "mcp-filesystem",\n      "args": ["--root", "/home/user/projects"]\n    }\n  }\n}',
    "config/channels.toml":
      '[web]\nenabled = true\nport = 3001\n\n[discord]\nenabled = false\ntoken_ref = "secret:discord_token"\n\n[telegram]\nenabled = true\ntoken_ref = "secret:telegram_token"\nchat_id = "123456789"\n',
    "config/agent-card.json": JSON.stringify(
      {
        name: "Residuum agent",
        description: "A personal AI agent, reachable over the Agent2Agent (A2A) protocol.",
        skills: [
          {
            id: "research",
            name: "Research",
            description: "Look into a topic across the web and memory, then report back.",
            tags: ["research"],
          },
        ],
      },
      null,
      2,
    ),
    "team/wiki/index.md":
      '---\nokf_version: "0.1"\n---\n\n# Wiki Index\n\n- [projects](projects/index.md) — active projects and their status\n',
    "team/wiki/log.md":
      "# Wiki Log\n\n- 2026-03-09: ingest — filed 3 pages from episodes ep-041..ep-043\n- 2026-03-05: lint — fixed stale frontmatter on projects/residuum.md\n",
    "team/wiki/projects/index.md":
      "---\ntype: index\ntitle: Projects\n---\n\n# Projects\n\n- [residuum](residuum.md) — personal agent framework\n",
    "team/wiki/projects/residuum.md":
      "---\ntype: concept\ntitle: Residuum\ndescription: Personal agent framework the user is building.\ntags: [project, rust]\nstatus: stable\nsources:\n  - episode: ep-041\nlast_modified: 2026-03-09\nstale_after: 2026-06-09\n---\n\n# Residuum\n\nA personal AI agent framework focused on genuine autonomy and persistent memory.\n",
    "memory/observations.jsonl":
      '{"text":"User prefers concise communication","timestamp":"2026-03-09T10:00:00Z","score":0.92}\n{"text":"Notification routing: Discord for urgent, Telegram for daily","timestamp":"2026-03-08T14:30:00Z","score":0.89}\n',
    "memory/reflections.jsonl":
      '{"text":"User is building a personal agent framework focused on genuine autonomy and persistent memory","timestamp":"2026-03-09T12:00:00Z","observations":5}\n',
  };
}
