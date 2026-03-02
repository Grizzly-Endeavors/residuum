<p align="center">
  <img src="assets/images/Residuum-logo.png" alt="Residuum" width="200">
</p>

<h1 align="center">Residuum</h1>

<p align="center">
  <strong>What Remains</strong>
</p>

<p align="center">
  <a href="https://residuum.bearflinn.com">Website</a> &middot;
  <a href="docs/">Docs</a> &middot;
  <a href="#quick-start">Quick Start</a> &middot;
  <a href="CONTRIBUTING.md">Contributing</a>
</p>

---

## The Problem

AI agent frameworks are built on sessions. Start a conversation, do some work, close it. Start another one tomorrow and re-explain everything. Some try to patch this with RAG or pinned memory files, but the model still treats each conversation as an isolated event.

## What Residuum Does

**No sessions.** One agent, one continuous thread. Residuum compresses your conversation history into a dense observation log that lives in context *at all times*. It doesn't retrieve what you talked about last week — it already has it. When it needs the full details of an older conversation, it knows exactly where to look.

**No boundaries.** CLI, Discord, webhooks — all channels feed the same agent, the same memory, the same thread. Message it from your terminal, pick up from Discord on your phone. The conversation never stopped.

**No wasted tokens.** Proactivity doesn't mean burning frontier-model calls to ask "is there anything to do?" every thirty minutes. Residuum uses structured pulse scheduling — you define what to check, when, and where to send results. The LLM fires when a check is due, runs on a cheap model, stays silent when there's nothing to report. Email scans, deployment checks, daily reviews — each one is a few lines of YAML.

## It Grows

**Skills & MCP** | Integrate with the tools and workflows you're already using.

**Self-evolution** | Baked in presets to periodically improve itself, how it responds, and how it integrates into your life. (Can be disabled, just ask your agent)

**Background work** | Delegate tasks and walk away. Your agent works independently and pings you when there's something worth knowing.

## Quick Start

```bash
curl -fsSL https://residuum.bearflinn.com/install | sh
residuum serve
```

First run handles API key setup and preferences. Web interface or terminal — web is better for initial config.

```bash
residuum setup  # terminal alternative
```

Once running, just talk to it.

## Building

Rust 1.85+, one supported API key.

```bash
git clone https://github.com/grizzly-endeavors/residuum.git
cd residuum
cargo build --release
```

```bash
cargo test --quiet
```

Pre-commit hooks enforce formatting, linting, and tests. [Architecture docs](docs/residuum-design.md) cover structure and design decisions.

## License

MIT — see [LICENSE](LICENSE).
