<p align="center">
  <img src="assets/images/Residuum-logo.png" alt="Residuum" width="200">
</p>

<h1 align="center">Residuum</h1>

<p align="center">
  <strong>Ready when you are.</strong>
</p>

<p align="center">
  <a href="https://residuum.dev">Website</a> &middot;
  <a href="docs/">Docs</a> &middot;
  <a href="#quick-start">Quick Start</a> &middot;
  <a href="#contributing">Contributing</a>
</p>

---

## The Problem

AI agent frameworks are built on sessions. Start a conversation, do some work, close it. Start another one tomorrow and re-explain everything. Some try to patch this with RAG or pinned memory files, but the model still treats each conversation as an isolated event.

The frameworks that do try persistence are busy chasing each other — bolting on every integration, every protocol, every feature they can find. They do everything. They remember nothing.

## What Residuum Does

**No sessions.** One agent, one continuous thread. Residuum compresses your conversation history into a dense observation log that lives in context *at all times*. It doesn't retrieve what you talked about last week — it already has it. When it needs the full details of an older conversation, it knows exactly where to look.

**No context switching.** Projects are scoped workspaces — notes, references, tools, MCP servers, all contained. Switch topics and the agent activates the relevant context, puts away what's not needed. A lightweight index of everything means it always knows what exists and can switch instantly.

**No wasted tokens.** Proactivity doesn't mean burning frontier-model calls to ask "is there anything to do?" every thirty minutes. Residuum uses structured pulse scheduling — you define what to check, when, and where to send results. The LLM fires when a check is due, runs on a cheap model, stays silent when there's nothing to report. Email scans, deployment checks, daily reviews — each one is a few lines of YAML.

**No boundaries.** CLI, Discord, webhooks — all channels feed the same agent, the same memory, the same thread. Message it from your terminal, pick up from Discord on your phone. The conversation never stopped.

**No black boxes.** Every piece of state is a file you can open in a text editor. Memory, personality, schedules, routing, project notes — markdown, YAML, TOML. One [pragmatic exception](docs/memory-search-design.md). If you want to know what your agent knows, look. If you want to change something, edit.

**No dependencies on anyone else.** Single binary, your machine, your API keys. No accounts, no telemetry, no cloud.

## Quick Start

```bash
curl -fsSL https://residuum.dev/install | sh
residuum serve
```

First run handles API key setup and preferences. Web interface or terminal — web is better for initial config.

```bash
residuum init  # terminal alternative
```

Once running, just talk to it.

## It Grows

**Skills** — drop a markdown file into `skills/` and the agent uses it when relevant. Code review checklists, deployment workflows, domain knowledge. Global or per-project.

**[MCP](https://modelcontextprotocol.io/)** — connect to external services. Web search, file systems, APIs, databases. Global or per-project.

**Scheduled actions** — the agent creates its own reminders and deferred tasks. "Follow up on that PR tomorrow at 10am." Done.

**Notification routing** — each pulse declares where results go. Push, feed, inbox, or any combination. Tell the agent to stop pinging you about something and it updates the routing. Everything visible in YAML.

## Building

Rust 1.85+, one supported API key.

```bash
git clone https://github.com/bear-revels/residuum.git
cd residuum
cargo build --release

# With Discord support
cargo build --release --features discord
```

```bash
cargo test --quiet
```

Pre-commit hooks enforce formatting, linting, and tests. [Architecture docs](docs/design.md) cover structure and design decisions.

## License

MIT — see [LICENSE](LICENSE).
