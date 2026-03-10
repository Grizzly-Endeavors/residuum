<p align="center">
  <img src="assets/images/banner.webp" alt="Residuum" width="100%">
</p>

<h3 align="center"><em>What Remains</em></h3>

<p align="center">
  <a href="https://github.com/Grizzly-Endeavors/residuum/actions/workflows/ci.yml">
    <img src="https://github.com/Grizzly-Endeavors/residuum/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI">
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

**A personal AI agent framework that eliminates session boundaries.** One agent, continuous memory, every channel.

## The Problem

AI agent frameworks are built on sessions. Start a conversation, do some work, close it. Start another one tomorrow and re-explain everything. Some try to patch this with RAG or pinned memory files, but the model still treats each conversation as an isolated event.

## What Residuum Does

**No sessions.** One agent, one continuous thread. Residuum compresses your conversation history into a dense observation log that lives in context *at all times*. It doesn't retrieve what you talked about last week — it already has it. When it needs the full details of an older conversation, it knows exactly where to look.

**No boundaries.** CLI, Discord, webhooks — all channels feed the same agent, the same memory, the same thread. Message it from your terminal, pick up from Discord on your phone. The conversation never stopped.

**No wasted tokens.** Proactivity doesn't mean burning frontier-model calls to ask "is there anything to do?" every thirty minutes. Residuum uses structured pulse scheduling — you define what to check, when, and where to send results. The LLM fires when a check is due, runs on a cheap model, stays silent when there's nothing to report. Email scans, deployment checks, daily reviews — each one is a few lines of YAML.

## How It's Different

**vs. [OpenClaw](https://github.com/openclaw/openclaw)** — OpenClaw established the personal agent pattern and Residuum builds on its architecture (gateway, channel normalization, file-first workspace, skill format). But OpenClaw has a memory cliff — context older than two days requires the agent to guess it should search. Its heartbeat fires a full LLM turn every 30 minutes to read a checklist, burning tokens on scheduling logic. Residuum solves both: observational memory keeps history in context continuously, and YAML pulse scheduling moves timing logic out of the LLM entirely. Rust implementation, not a fork. OpenClaw-compatible skills.

**vs. [NanoClaw](https://github.com/qwibitai/nanoclaw)** — NanoClaw optimizes for minimalism and container isolation (~500 lines of TypeScript). Residuum optimizes for continuity — persistent memory compression, structured proactive scheduling, background task delegation with model tiering, and multi-channel convergence into a single thread. Different goals, different tradeoffs.

**vs. RAG-based agents** — No retrieval step for recent history. The observation log is always in context. Deep retrieval exists for older episodes via hybrid search (BM25 + vector embeddings), but working memory is continuous, not query-dependent.

## Core Systems

| System | What it does | Docs |
|--------|-------------|------|
| **Observational Memory** | Two-tier compression (Observer + Reflector) keeps conversation history in context at all times. No RAG latency, no retrieval misses. | [Design](docs/memory-search-design.md) |
| **Multi-Channel Gateway** | CLI, Discord, Telegram, webhooks — all channels feed the same agent, same memory, same thread. | [Architecture](docs/residuum-design.md) |
| **Pulse Scheduling** | YAML-defined proactive checks. Gateway handles timing; LLM fires only when work is due. Zero token cost when nothing is scheduled. | [Design](docs/background-tasks-design.md) |
| **SubAgent Tasks** | Delegate work to background agents with automatic model tiering — cheap models first, expensive fallback only when needed. | [Design](docs/background-tasks-design.md) |
| **Projects** | Scoped knowledge folders with notes, references, and project-specific skills. Agent activates relevant context automatically. | [Design](docs/projects-context-design.md) |
| **Skills & MCP** | Extensible tool system with Model Context Protocol integration. OpenClaw-compatible skill format. | [Architecture](docs/residuum-design.md) |
| **Notification Routing** | Declarative channel routing via CHANNELS.yml — control where alerts land, per-task. | [Design](docs/notification-routing-design.md) |

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

The short version: fork, branch from `dev`, make your changes, open a PR. Pre-commit hooks handle formatting and linting. CI must pass before merge.

## License

MIT — see [LICENSE](LICENSE).
