<p align="center">
  <img src="assets/images/Residuum_logo.webp" alt="Residuum" width="200">
</p>

<h1 align="center">Residuum</h1>

<p align="center">
  <strong>One Agent, for <em>You</em>.</strong>
</p>

<p align="center">
  Memory that lasts. Capabilities that grow with you. Proactivity without the price tag.
</p>

<p align="center">
  <a href="https://residuum.bearflinn.com">Website</a> &middot;
  <a href="docs/">Docs</a> &middot;
  <a href="#quick-start">Quick Start</a> &middot;
  <a href="#contributing">Contributing</a>
</p>

---

## The Problem

Every AI agent framework gives you a chatbot with amnesia. Start a new session, re-explain your project, re-describe your preferences, hope it remembers what you told it yesterday. Some try RAG over old transcripts. Some let you pin facts to a memory file. None of them feel like talking to something that actually *knows you*.

And proactivity? Most frameworks burn frontier-model tokens every 30 minutes to ask "is there anything to do?" — then answer "no" 95% of the time. That's not proactivity, it's a money pit.

Residuum takes a different approach.

## What Residuum Actually Does

**It remembers.** Not through a knowledge graph you have to maintain, or a RAG pipeline that might retrieve the right thing if it guesses the right query. Residuum continuously compresses your conversation history into a dense, chronological observation log that lives in the agent's context window *at all times*. Your agent doesn't have to decide to search for something — it already knows what you've been working on this week, what you decided last Tuesday, and what's still unfinished. When it needs the full details of an older conversation, it knows exactly which episode to pull up.

**It stays organized.** Tell your agent about a new side project, and it creates a scoped workspace — notes, references, tools, MCP servers, all contained. When you switch topics, the agent activates the relevant project context and puts away what's not needed. You never carry token cost for context that isn't relevant to what you're doing right now. Your agent maintains a lightweight index of *everything* — it always knows what projects exist and can switch instantly.

**It acts on its own — cheaply.** Instead of waking a frontier model to check if there's anything to do, Residuum uses structured pulse scheduling. You define *what* to check, *when* to check it, and *where* to send the results. The gateway handles all timing. The LLM only fires when a check is actually due, runs on a cheap model, and stays silent when there's nothing to report. Want your agent to scan your email every hour during work hours? Check deployment status every 5 minutes? Review your day at 8am? Each one is a few lines of YAML.

**It reaches you where you are.** CLI, Discord, webhooks — all channels feed the same agent, the same memory, the same conversation. Message it from your terminal at work, pick up the thread from Discord on your phone. There are no sessions to start or contexts to rebuild.

**It's transparent.** Every piece of agent state is a file you can open in a text editor. The agent's memory, personality, scheduled tasks, notification routing, project notes — all plain text. Markdown, YAML, TOML. No opaque databases, no hidden embeddings (with one [pragmatic exception](docs/memory-search-design.md)). If you want to know what your agent knows, look at the files. If you want to change something, edit them.

**It's yours.** Single binary on your machine, your API keys, your data stays local. No accounts, no telemetry, no cloud dependency.

## Quick Start

```bash
# Install
curl -fsSL https://residuum.bearflinn.com/install | sh

# Start the gateway
residuum serve
```

Open the local web interface to configure your API keys and preferences, or use the onboarding wizard from the terminal. (Web is a better experience)

## Growing With You

Residuum isn't a tool you configure once and use — it's an agent that adapts as you work with it.

**Skills** extend the agent's capabilities without code changes. Drop a markdown file into `skills/` and the agent activates it when relevant — code review checklists, deployment workflows, domain-specific knowledge. Skills can be global or scoped to a specific project.

**[MCP](https://modelcontextprotocol.io/) support** connects your agent to external services. Web search, file systems, APIs, databases — configure MCP servers globally or per-project, and their tools are available alongside built-in ones.

**Scheduled actions** let the agent create its own one-shot reminders and deferred tasks. "Remind me to follow up on that PR tomorrow at 10am" — the agent schedules it, it fires, it's done.

**Notification routing** puts you in control of how results reach you. Each pulse declares where its results go — push notifications, the agent's feed, a silent inbox, or any combination. Tell the agent "stop pinging me about PR reviews" and it updates the routing. Everything is visible in YAML files you can read and override.

### Why Rust?

> Single binary, no runtime dependencies, lightweight, reliable, blah blah blah. Rust wasn't the right choice for this project. I had to rebuild boilerplate that there are existing libraries for, cut myself off from a lot of extensibility options, and the single-user target means it doesn't get most of the benefits of using the language. But I ***Really*** hate Python and TypeScript. I've tried both in many projects, but I was cursed by a computer science major who convinced me to write my first coding project in Rust, and now I can't stand anything else.

### Building from Source

```bash
git clone https://github.com/bear-revels/residuum.git
cd residuum
cargo build --release

# With Discord support
cargo build --release --features discord
```

Requires Rust 1.85+ (2024 edition) and an API key for at least one supported provider.

### Testing

```bash
cargo test --quiet
```

Pre-commit hooks enforce formatting, linting, and tests on every commit. See the [architecture docs](docs/residuum-design.md) for project structure and design decisions.

## License

MIT License — see [LICENSE](LICENSE) for details.
