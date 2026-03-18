<p align="center">
  <img src="assets/images/banner.webp" alt="Residuum" width="100%">
</p>

<h3 align="center"><em>What Remains</em></h3>

<p align="center">
  <a href="https://github.com/Grizzly-Endeavors/residuum/actions/workflows/release.yml">
    <img src="https://github.com/Grizzly-Endeavors/residuum/actions/workflows/release.yml/badge.svg" alt="CI">
  </a>
  <a href="https://github.com/Grizzly-Endeavors/residuum/releases/latest">
    <img src="https://img.shields.io/github/v/release/Grizzly-Endeavors/residuum?label=release&color=blue" alt="Latest Release">
  </a>
  <a href="LICENSE">
    <img src="https://img.shields.io/badge/license-MIT-green" alt="MIT License">
  </a>
  <img src="https://img.shields.io/badge/rust-1.85%2B-orange?logo=rust" alt="Rust 1.85+">
</p>

<p align="center">
  <a href="https://agent-residuum.com">Website</a> &middot;
  <a href="docs/">Documentation</a> &middot;
  <a href="#quick-start">Quick Start</a> &middot;
  <a href="CONTRIBUTING.md">Contributing</a>
</p>

---

**Agents that last.**

Residuum is an AI agent with continuous memory, multi-channel access, and proactive scheduling. Install it, talk to it, come back tomorrow — it already knows what you were working on.

## What It Does

### No more re-explaining yourself

Your agent carries a compressed history of every conversation. Come back after a week — it knows what you were working on, what decisions were made, what's still open. When it needs older details, it searches its memory automatically.

### No more juggling apps

CLI, Discord, Telegram, webhooks — all feed the same agent, same memory, same thread. Start a thought on your terminal, pick it up on your phone. The conversation never splits.

### No more checking things yourself

Define what to check and when — email scans, deployment status, daily reviews. The agent handles the routine on a schedule, stays silent when there's nothing to report, and routes results where you want them.

## Features

- **Observational memory** — Compressed conversation history stays in context at all times. Older details are searchable by keyword, date, or project. [docs →](docs/memory-search-design.md)
- **Multi-channel** — CLI, Discord, Telegram, webhooks. Talk to it from anywhere, same conversation. [docs →](docs/residuum-design.md)
- **Pulse scheduling** — Scheduled checks defined in YAML. The agent fires when work is due, stays quiet otherwise. [docs →](docs/background-tasks-design.md)
- **Background tasks** — Hand it something and walk away. Cheap models handle routine work; expensive ones step in when needed. [docs →](docs/background-tasks-design.md)
- **Projects** — Scoped knowledge folders with notes, references, and project-specific tools. The agent activates relevant context automatically. [docs →](docs/projects-context-design.md)
- **Skills & MCP** — Extensible tools with Model Context Protocol integration. [docs →](docs/residuum-design.md)
- **Notification routing** — Control where alerts land — agent, inbox, push, webhooks — via simple YAML config. [docs →](docs/notification-routing-design.md)

## Quick Start

### Install

```bash
curl -fsSL https://agent-residuum.com/install | sh
```

Detects your platform automatically. Supports Linux (x86_64, aarch64) and macOS (Apple Silicon).

### First Run

```bash
residuum serve
```

On first launch, a web UI opens for initial setup — API keys, personality preferences, and channel connections.

```bash
residuum setup  # terminal alternative
```

Once running, just talk to it.

### Build from Source

Requires Rust 1.85+ and one supported LLM API key.

```bash
git clone https://github.com/Grizzly-Endeavors/residuum.git
cd residuum
cargo build --release
```

Pre-commit hooks enforce formatting, linting, and tests on every commit. See [Architecture docs](docs/residuum-design.md) for structure and design decisions.

## Supported Providers

- **Anthropic** (Claude)
- **OpenAI** (GPT-4o, o-series)
- **Google** (Gemini)
- **Ollama** (local models)

Provider failover is built in — configure a primary and fallback.

## Design Philosophy

- **File-first** — All state lives in human-readable files you can inspect, edit, and version control. No opaque databases.
- **Right work, right place** — The gateway handles scheduling and file watching. The LLM handles judgment.
- **Independent systems** — Memory, Projects, Pulses, and Skills are designed independently. They compose through shared data, not tight coupling.

Full rationale: [Design Philosophy](docs/design-philosophy.md)

## Documentation

- [Architecture & System Design](docs/residuum-design.md)
- [Design Philosophy](docs/design-philosophy.md)
- [Memory & Search](docs/memory-search-design.md)
- [Projects Context](docs/projects-context-design.md)
- [Background Tasks](docs/background-tasks-design.md)
- [Notification Routing](docs/notification-routing-design.md)
- [Systems Usage Guide](docs/systems-usage/)

## Contributing

Contributions are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md) for the full workflow, code standards, and lint rules.

The short version: fork, branch from `main`, make your changes, open a PR. Pre-commit hooks handle formatting and linting. CI must pass before merge.

## License

MIT — see [LICENSE](LICENSE).
