import type { RecentMessage } from "../../src/lib/types";
import type { MockClock } from "../env";

// Recent live messages — span today, yesterday, and the day before so the
// frontend inserts a day divider between each run. These correspond to the
// contents of recent_messages.json on the backend.
export function sampleRecentMessages(clock: MockClock): RecentMessage[] {
  // A given number of calendar days before now at a specific hour, so the
  // messages exercise the day-divider logic in the frontend feed store.
  const daysAgoAt = clock.dayAt;
  return [
    // Main's reply to the relay that closes ep-003: the turn began in that
    // episode, so it's shown once the episode loads.
    {
      role: "assistant",
      content: "Noted. The observer notes are in; I'll fold them into the memory doc.",
      timestamp: daysAgoAt(2, 9, 0),
      visibility: "background",
    },
    {
      role: "user",
      content: "Did the observer flag anything odd in last night's batch?",
      timestamp: daysAgoAt(2, 9, 12),
      visibility: "user",
    },
    {
      role: "assistant",
      content:
        "Nothing unusual. The observer compressed 14 messages into `ep-003` " +
        "around 02:00 local time and logged two fresh reflections. Memory " +
        "utilization is holding at ~38% of the context window.",
      timestamp: daysAgoAt(2, 9, 13),
      visibility: "user",
    },
    {
      role: "user",
      content: "Can you check the current memory stats?",
      timestamp: daysAgoAt(1, 14, 20),
      visibility: "user",
    },
    {
      role: "assistant",
      content: "Let me look at the memory subsystem status.",
      tool_calls: [
        {
          id: "tc_mock_stats",
          name: "exec",
          arguments: { command: "residuum memory stats" },
        },
      ],
      timestamp: daysAgoAt(1, 14, 20),
      visibility: "user",
    },
    {
      role: "tool",
      content:
        "Context window: 12,847 / 200,000 tokens (6.4%)\n" +
        "Memory observations: 42\n" +
        "Reflections: 8\n" +
        "Last observer run: 3 minutes ago",
      tool_call_id: "tc_mock_stats",
      timestamp: daysAgoAt(1, 14, 21),
      visibility: "user",
    },
    {
      role: "assistant",
      content:
        "Here are the current memory stats:\n\n" +
        "- **Context window**: 12,847 / 200,000 tokens (6.4%)\n" +
        "- **Observations**: 42 stored\n" +
        "- **Reflections**: 8 synthesized\n" +
        "- **Last observer run**: 3 minutes ago\n\n" +
        "The context is well within limits. The observer will run again " +
        "once we cross the 30k token threshold.",
      timestamp: daysAgoAt(1, 14, 22),
      visibility: "user",
    },
    {
      role: "user",
      content: "Good. Let's keep iterating on the notification routing doc.",
      timestamp: daysAgoAt(0, 10, 5),
      visibility: "user",
    },
    {
      role: "assistant",
      content:
        "Picking up where we left off. I've got the three-tier priority " +
        "model (`urgent`, `normal`, `low`) and the per-context channel " +
        "overrides drafted. Next up: the fallback behaviour when a channel " +
        "is unreachable. Want me to start there?",
      timestamp: daysAgoAt(0, 10, 6),
      visibility: "user",
    },
    // The owner pasting an agent header by hand: stays their own message.
    {
      role: "user",
      content:
        "[Agent Message from spawned-research-3f9a (spawned)]\n" +
        "Pasting this header myself to see what the UI does with it.",
      timestamp: daysAgoAt(0, 10, 10),
      visibility: "user",
    },
    // Background noise from before sessions existed: stays hidden.
    {
      role: "user",
      content: "Pulse check: inbox_check. Review the inbox for anything urgent.",
      timestamp: daysAgoAt(0, 10, 30),
      visibility: "background",
    },
    {
      role: "assistant",
      content: "HEARTBEAT_OK",
      timestamp: daysAgoAt(0, 10, 30),
      visibility: "background",
    },
    // A spawned session's relayed result and main's reply: shown.
    {
      role: "user",
      content:
        "[Agent Message from spawned-research-3f9a (spawned)]\n" +
        "Found three fallback strategies worth comparing:\n\n" +
        "1. **Retry with backoff** on the same channel, capped at 3 attempts.\n" +
        "2. **Cascade** to the next channel in the priority list.\n" +
        "3. **Park** the notification in the inbox and surface it on next contact.\n\n" +
        "Cascade is what most setups expect; parking is the safest default when every channel is down. " +
        "Sources and notes are in `team/wiki/notification-fallbacks.md`.",
      timestamp: daysAgoAt(0, 10, 41),
      visibility: "background",
      agent_sender: { address: "spawned-research-3f9a", category: "spawned" },
    },
    {
      role: "assistant",
      content:
        "The research session came back: cascade first, then park in the inbox if every " +
        "channel is down. I'll draft the fallback section that way.",
      timestamp: daysAgoAt(0, 10, 42),
      visibility: "background",
    },
  ];
}

// Sample episodes — older compressed history that the frontend fetches
// lazily as the user scrolls to the top. The cursor chain is
// ep-003 → ep-002 → ep-001 → null.
export interface SampleEpisode {
  id: string;
  /** YYYY-MM-DD */
  date: string;
  messages: RecentMessage[];
}

export function sampleEpisodes(clock: MockClock): SampleEpisode[] {
  const isoDateDaysAgo = clock.dateDaysAgo;
  return [
    {
      id: "ep-003",
      date: isoDateDaysAgo(3),
      messages: [
        {
          role: "user",
          content: "Walk me through what the observer actually stores vs. what it drops.",
          timestamp: `${isoDateDaysAgo(3)}T00:00:00.000Z`,
          visibility: "user",
        },
        {
          role: "assistant",
          content:
            "The observer keeps three things for each compression pass:\n\n" +
            "1. **Observations** — atomic facts extracted from the chat, stored in the memory index.\n" +
            "2. **Reflections** — higher-order patterns it synthesises across observations.\n" +
            "3. **Episode transcript** — the raw JSONL of the messages it compressed, tagged with the episode id.\n\n" +
            "What it drops is the _surface wording_ of the messages — it " +
            "remembers the substance but won't be able to quote verbatim.",
          timestamp: `${isoDateDaysAgo(3)}T00:00:00.000Z`,
          visibility: "user",
        },
        // Episodes don't record visibility; the structured sender marks this
        // as a session's message. Main's reply opens the recent segment.
        {
          role: "user",
          content:
            "[Agent Message from scheduled-observer-audit-2c41 (scheduled)]\n" +
            "Observer audit done: nothing was dropped that should have been kept.",
          timestamp: `${isoDateDaysAgo(3)}T00:00:00.000Z`,
          visibility: "user",
          agent_sender: { address: "scheduled-observer-audit-2c41", category: "scheduled" },
        },
      ],
    },
    {
      id: "ep-002",
      date: isoDateDaysAgo(5),
      messages: [
        {
          role: "user",
          content: "How do I direct you at a specific episode when we talk?",
          timestamp: `${isoDateDaysAgo(5)}T00:00:00.000Z`,
          visibility: "user",
        },
        {
          role: "assistant",
          content:
            "Reference the episode id directly — e.g. `ep-002` — and I'll " +
            "pull the relevant observations from memory. You can also scope " +
            "by date, which is often easier if you don't remember the id.",
          timestamp: `${isoDateDaysAgo(5)}T00:00:00.000Z`,
          visibility: "user",
        },
        {
          role: "user",
          content: "That's perfect. Let's make it visible in the UI too.",
          timestamp: `${isoDateDaysAgo(5)}T00:00:00.000Z`,
          visibility: "user",
        },
      ],
    },
    {
      id: "ep-001",
      date: isoDateDaysAgo(8),
      messages: [
        {
          role: "user",
          content: "First conversation of the week — let's set goals.",
          timestamp: `${isoDateDaysAgo(8)}T00:00:00.000Z`,
          visibility: "user",
        },
        {
          role: "assistant",
          content:
            "Three things on the board:\n\n" +
            "- Finish the lazy-loaded chat history feature.\n" +
            "- Tighten the notification routing doc.\n" +
            "- Revisit the observer thresholds once we have a week of data.\n\n" +
            "Anything missing?",
          timestamp: `${isoDateDaysAgo(8)}T00:00:00.000Z`,
          visibility: "user",
        },
      ],
    },
  ];
}

/** The replies a chat turn cycles through, in order. */
export const cannedResponses: readonly string[] = [
  "I've looked into that and here's what I found:\n\n" +
    "## Key Points\n\n" +
    "1. **Configuration** — The settings are stored in `config.toml` under the `[memory]` section\n" +
    "2. **Thresholds** — Observer triggers at 30k tokens, reflector at 40k\n" +
    "3. **Search** — Hybrid BM25 + vector search with configurable weights\n\n" +
    "```toml\n[memory]\nobserver_threshold_tokens = 30000\nreflector_threshold_tokens = 40000\n```\n\n" +
    "Would you like me to adjust any of these values?",

  "Great question! Let me break that down:\n\n" +
    "The notification system supports **three channels**:\n\n" +
    "- **Discord** — Real-time alerts via bot DM\n" +
    "- **Telegram** — Daily digest summaries\n" +
    "- **Webhook** — Custom HTTP POST for external integrations\n\n" +
    "Each channel can be configured independently. " +
    "The priority routing rules determine which channel receives which notifications.\n\n" +
    "> **Tip**: Use `secret:discord_token` syntax in your config to reference encrypted secrets.",

  "I've completed the analysis. Here's a summary:\n\n" +
    "### Performance Metrics\n\n" +
    "| Metric | Value | Status |\n" +
    "|--------|-------|--------|\n" +
    "| Response time | 1.2s avg | Good |\n" +
    "| Memory usage | 45MB | Normal |\n" +
    "| Token throughput | 850/s | Optimal |\n\n" +
    "Everything looks healthy. The memory subsystem is operating within expected parameters. " +
    "Let me know if you'd like a deeper dive into any specific area.",
];
