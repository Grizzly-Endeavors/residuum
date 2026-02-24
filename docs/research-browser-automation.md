# Browser Automation Research

**Date**: 2026-02-24
**Status**: Research / Pre-design

---

## Context

The ironclaw design doc lists `tools/browser.rs` as a planned built-in tool for "Browser automation (headless)." This document evaluates the landscape and recommends an approach that fits ironclaw's architecture.

### Why browser automation matters for ironclaw

The agent has use cases that simple HTTP fetch (`web_fetch`) cannot satisfy:

- **JS-rendered content**: SPAs, dashboards, and modern web apps that require JavaScript execution to produce meaningful content.
- **Authenticated web sessions**: Checking email inboxes, PR dashboards, calendar views, and other services where the agent needs to log in and interact.
- **Pulse task evaluation**: Heartbeat pulses like "check for urgent unread emails" or "any PRs waiting on my review?" often require navigating real web UIs.
- **Form interaction**: Filling out forms, submitting data, interacting with web applications on behalf of the user.
- **Data extraction from complex pages**: Tables, dynamic content, paginated results that require scrolling or clicking through.
- **Screenshots**: Visual verification, capturing state for reference files in projects.

### Constraints from ironclaw's architecture

- **Async-first**: Everything is tokio-based. Synchronous browser crates are a poor fit.
- **Tool trait**: Must implement `Tool` with `name()`, `definition()`, `execute()`. Returns `ToolResult` (text output + error flag).
- **Gated tool**: Browser should likely be gated (like `exec`), requiring project opt-in via `PROJECT.md` frontmatter.
- **MCP client exists**: ironclaw already has a full MCP client (`mcp/client.rs`, `mcp/transport.rs`, `mcp/lifecycle.rs`) supporting stdio and HTTP/SSE transports. MCP servers can be configured globally or per-project.
- **File-first**: Results should be inspectable. Screenshots saved to workspace, extracted data written to files.
- **No silent failures**: Every browser operation failure must be visible.
- **Token efficiency**: Browser output sent back to the LLM must be compact. Raw HTML is wasteful. Structured, text-based representations are preferred.

---

## Approach A: Playwright MCP Server

Use Microsoft's official `@playwright/mcp` as an MCP server, leveraging ironclaw's existing MCP infrastructure.

### How it works

The Playwright MCP server spawns as a child process (Node.js + Playwright). Communication happens via stdio using JSON-RPC 2.0 — exactly how ironclaw's MCP client already works. The server exposes 25+ tools for browser interaction.

**Key architectural feature**: Playwright MCP uses the browser's **accessibility tree** instead of screenshots for LLM interaction. The accessibility tree is a structured, text-based representation of a webpage (~2-5KB per snapshot vs ~500KB for a screenshot). This is inherently LLM-friendly — no vision model needed, deterministic element identification, low token cost.

### Configuration

Global (always available):
```toml
# config.toml
[mcp.servers.browser]
command = "npx"
args = ["@playwright/mcp@latest", "--headless"]
```

Or project-scoped (only when project is active):
```yaml
# PROJECT.md frontmatter
mcp_servers:
  - name: browser
    command: "npx"
    args: ["@playwright/mcp@latest", "--headless"]
```

### Tools exposed

The Playwright MCP server provides tools including:
- `browser_navigate` — Navigate to a URL
- `browser_snapshot` — Take an accessibility tree snapshot (primary interaction mode)
- `browser_screenshot` — Take a visual screenshot
- `browser_click` — Click an element (identified by role + accessible name from snapshot)
- `browser_type` — Type text into an element
- `browser_select_option` — Select from dropdowns
- `browser_hover` — Hover over elements
- `browser_drag` — Drag and drop
- `browser_press_key` — Keyboard input
- `browser_execute_javascript` — Execute arbitrary JS
- `browser_wait` — Wait for conditions
- `browser_tab_*` — Tab management (new, close, list, select)
- `browser_verify_*` — Verification tools for assertions
- `browser_close` — Close the browser

### Two operating modes

1. **Snapshot mode** (default): Uses accessibility tree for all interactions. Fast, deterministic, low token cost. Elements identified by `ROLE` and `ACCESSIBLE_NAME` from the snapshot.

2. **Vision mode**: Falls back to coordinate-based interaction for visual elements not in the accessibility tree (canvas, WebGL, complex visualizations). Requires vision-capable LLM.

3. **Hybrid mode** (`--vision auto`): Uses accessibility tree for ~90% of interactions, automatically switches to vision for elements the tree can't parse.

### Pros

- **Zero Rust code for browser logic.** The MCP server handles everything. ironclaw's existing MCP client connects to it without modification.
- **Accessibility tree approach is ideal for LLMs.** Compact structured data, no vision model dependency, deterministic element identification.
- **Battle-tested.** Microsoft-maintained, actively developed, weekly updates. Used by Claude Desktop, Cursor, VS Code Copilot.
- **Cross-browser.** Chromium, Firefox, WebKit support.
- **25+ tools out of the box.** Comprehensive coverage of browser interactions.
- **Hot-configurable.** Can be added per-project via `PROJECT.md` frontmatter without any ironclaw code changes.
- **Matches the existing pattern.** MCP servers are already how ironclaw extends capabilities dynamically.

### Cons

- **Node.js dependency.** Requires Node.js + npm installed on the host. This is the most significant trade-off — ironclaw is otherwise a self-contained Rust binary.
- **Process overhead.** Running a Node.js MCP server as a child process consumes more memory and startup time than a compiled-in tool.
- **Opaque error handling.** MCP tool errors come through as JSON-RPC error responses. Debugging browser failures requires looking at the MCP server's stderr, not ironclaw's logs.
- **No Rust-level control.** Can't customize browser behavior below the MCP tool abstraction. If the MCP server doesn't expose a capability, it's not available.
- **Shadow DOM limitations.** The accessibility tree approach struggles with Shadow DOM (common in design systems using Lit, Shoelace, etc.).

---

## Approach B: Native Rust Tool (CDP via chromey/chromiumoxide)

Implement browser automation as a built-in `Tool` in `src/tools/browser.rs` using a Rust CDP (Chrome DevTools Protocol) crate.

### Crate options

#### chromiumoxide (v0.7.0)
- **What**: High-level async Rust API over Chrome DevTools Protocol.
- **Runtime**: Supports both `async-std` and `tokio` (via `tokio-runtime` feature flag, requires `default-features = false`).
- **Maintenance**: Moderate. Last release on crates.io is 0.7.0. GitHub activity has slowed; forks have appeared.
- **Capabilities**: Navigate, JS execution, DOM interaction, screenshots, PDF generation, cookie management, network interception (via CDP events).
- **Architecture**: Uses `async-tungstenite` for WebSocket communication with Chrome. Auto-generates all CDP domain types from PDL files (~60K lines of generated Rust code — expect slow initial compile).
- **Chrome requirement**: Needs Chrome/Chromium installed, or can auto-download via `chromiumoxide_fetcher`.

#### chromey (v2.37.158)
- **What**: Actively maintained fork of chromiumoxide, originating from the Spider web crawler project.
- **Runtime**: **Tokio-native** (uses `tokio-tungstenite` directly, no async-std baggage).
- **Maintenance**: Very active. Version numbers suggest frequent releases. Keeps CDP bindings up to date.
- **Improvements over upstream**: Bug fixes, improved emulation, adblocking/firewall support, high-concurrency CDP capabilities, performance optimizations.
- **Best fit for ironclaw** if going the native CDP route, given tokio-native design and active maintenance.

#### headless_chrome (v1.0.17)
- **What**: Rust equivalent of Puppeteer. High-level API over CDP.
- **Runtime**: **Synchronous**. Uses plain threads, not async.
- **Maintenance**: Active (regular releases through 2025).
- **Disqualified for ironclaw**: Synchronous API is fundamentally incompatible with ironclaw's async-first architecture. Would require `spawn_blocking` wrappers everywhere, losing the benefits of async.

#### fantoccini (v0.22.0)
- **What**: WebDriver protocol client for Rust.
- **Runtime**: **Tokio-native**. Well-maintained by jonhoo (Jon Gjengset).
- **Maintenance**: Very active. Regular releases (0.22.0 in June 2025).
- **Protocol**: Uses **WebDriver** (W3C standard), not CDP. Requires a separate WebDriver server (geckodriver, chromedriver, etc.) to be running.
- **Capabilities**: Navigation, element interaction, JS execution, screenshots, cookies. Does NOT support CDP-specific features (network interception, JS coverage, performance tracing).
- **Trade-off**: More portable (works with Firefox, Chrome, Safari via WebDriver) but less capable than CDP-based approaches. The external driver dependency adds operational complexity.

### Recommended crate: chromey

If building a native tool, `chromey` is the best fit:
- Tokio-native (no runtime mismatch)
- Actively maintained fork with frequent updates
- Full CDP access for advanced capabilities
- Inherits chromiumoxide's well-designed async API

### Implementation sketch

```rust
// src/tools/browser.rs

pub struct BrowserTool {
    // Lazily initialized browser instance
    browser: Arc<Mutex<Option<Browser>>>,
}

#[async_trait]
impl Tool for BrowserTool {
    fn name(&self) -> &'static str { "browser" }

    fn definition(&self) -> ToolDefinition {
        // Parameters: action (navigate/click/type/screenshot/extract/js),
        //             url, selector, text, script, etc.
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        // Dispatch based on "action" field
        // Return structured text output, not raw HTML
    }
}
```

### LLM-friendly output design

The key challenge with a native tool is designing the output format. Raw HTML is wasteful. Options:

1. **Text extraction**: Strip tags, return visible text content. Simple but loses structure.
2. **Simplified DOM**: Return a condensed representation with element roles, text content, and selectable attributes. Mimics the accessibility tree approach.
3. **Structured JSON**: Return JSON with page title, headings, links, form fields, tables as structured data.
4. **Accessibility tree**: Use CDP's `Accessibility.getFullAXTree` to get the same accessibility tree that Playwright MCP uses.

Option 4 (accessibility tree via CDP) would give the same LLM-friendly output as the Playwright MCP approach while staying Rust-native.

### Pros

- **No external runtime dependency.** Compiled into the binary. Only needs Chrome/Chromium installed (which is needed regardless).
- **Full control.** Can customize behavior, error handling, output format, resource limits.
- **Lower overhead.** No child process, no JSON-RPC serialization, no Node.js memory.
- **Consistent error handling.** Errors flow through ironclaw's `ToolError` system, visible in tracing logs.
- **Gating integrates naturally.** Just another gated tool in the `ToolFilter`.

### Cons

- **Significant implementation effort.** Need to build the tool, design the output format, handle browser lifecycle, manage tabs, implement timeout/cleanup logic.
- **Maintenance burden.** Browser automation is a moving target. CDP changes, Chrome updates break things, edge cases are numerous.
- **Compile time impact.** chromey/chromiumoxide generates ~60K lines of CDP bindings. Adds meaningful compile time.
- **Reinventing the wheel.** Playwright MCP has years of engineering behind its LLM-friendly output format. A native tool would need to replicate that quality.
- **Chrome-only.** CDP is Chrome/Chromium specific. No Firefox or WebKit support without additional work.

---

## Approach C: Hybrid (Thin Native Tool + MCP for Advanced)

Combine a minimal native browser tool for common operations with MCP for advanced automation.

### Design

**Native tool** (`browser`): Handles the 80% case — navigate to a URL, extract text content, take a screenshot, execute a JS snippet. Uses `chromey` for CDP. Designed for pulse tasks and quick lookups.

**MCP server** (Playwright): Available for projects that need full browser automation — form filling, multi-step workflows, complex element interaction. Configured per-project.

### Tool definition

```
browser tool:
  - navigate: Go to URL, return page text content (accessibility tree or simplified DOM)
  - screenshot: Save screenshot to workspace, return file path
  - js: Execute JavaScript, return result
  - extract: Extract structured data (tables, links, headings) from current page
```

For anything beyond these four operations, the agent uses the Playwright MCP server's full 25+ tool suite.

### Pros

- **Quick operations stay fast.** Simple fetch-and-extract doesn't need a Node.js process.
- **Full capability when needed.** Playwright MCP handles complex automation.
- **Progressive complexity.** Projects can start with just the native tool and add Playwright MCP when needed.

### Cons

- **Two browser instances.** The native tool and MCP server manage separate browser processes unless carefully coordinated.
- **Inconsistent APIs.** The agent sees different tool interfaces for "simple" vs "complex" browser operations.
- **More code paths to maintain.** Both the native implementation and the MCP configuration.
- **Confusing for the agent.** When should it use `browser` vs Playwright MCP tools? This is a judgment call the LLM may get wrong.

---

## Approach D: Skill-Based Browser Automation

Instead of a built-in tool or MCP server, provide browser automation as a **Skill** that the agent activates when needed.

### Design

A bundled skill (`skills/browser-automation/SKILL.md`) that:
1. Instructs the agent on how to use browser automation.
2. Specifies `allowed-tools: [exec]` to enable shell commands.
3. Includes scripts that wrap Playwright CLI or a headless browser.
4. The agent calls `exec` to run browser scripts and reads the output.

### Example

```markdown
---
name: browser-automation
description: "Automate browser interactions via Playwright CLI"
allowed-tools:
  - exec
---

## Usage

Use the `exec` tool to run Playwright scripts for browser automation.

### Navigate and extract text
exec: npx playwright-cli navigate --url "URL" --extract text

### Take screenshot
exec: npx playwright-cli screenshot --url "URL" --output workspace/screenshot.png

### Run custom script
Write a Playwright script to `workspace/browser-script.js`, then:
exec: npx playwright test workspace/browser-script.js
```

### Pros

- **Zero implementation in ironclaw.** Pure skill + existing `exec` tool.
- **Flexible.** The skill can evolve independently of ironclaw releases.
- **User-customizable.** Users can modify the skill to use different tools.

### Cons

- **Fragile.** Shell-based automation with string parsing is error-prone.
- **Poor error handling.** Errors from `exec` are raw stderr, not structured.
- **No persistent browser session.** Each `exec` call spawns a new process. No state between calls.
- **Token-expensive.** Multi-step browser interactions require many tool calls with verbose output.
- **Still requires Node.js/Playwright installed.**

---

## Comparison Matrix

| Criterion | A: Playwright MCP | B: Native (chromey) | C: Hybrid | D: Skill |
|-----------|-------------------|---------------------|-----------|----------|
| **Implementation effort** | None (config only) | High | Very high | Low |
| **Maintenance burden** | Low (Microsoft maintains) | High | Very high | Low |
| **External dependencies** | Node.js + Playwright | Chrome only | Both | Node.js + Playwright |
| **LLM output quality** | Excellent (accessibility tree) | Needs design work | Mixed | Raw text/stderr |
| **Token efficiency** | Good (2-5KB snapshots) | Depends on design | Good for simple, good for complex | Poor |
| **Startup latency** | ~2-3s (Node.js spawn) | ~0.5s (Chrome launch) | Both | ~2-3s per exec call |
| **Persistent browser session** | Yes (MCP server lifecycle) | Yes (tool state) | Partially | No |
| **Cross-browser** | Yes (Chromium, Firefox, WebKit) | No (Chrome only) | Partially | Yes |
| **Fits ironclaw patterns** | MCP is a first-class pattern | Tool is a first-class pattern | Awkward overlap | Skill is a first-class pattern |
| **Error visibility** | MCP error responses | Full control | Mixed | Raw stderr |
| **Gating** | Per-project MCP config | ToolFilter gating | Both | Skill activation |
| **Compile time impact** | None | Significant (~60K LOC generated) | Significant | None |

---

## Recommendation

**Primary: Approach A (Playwright MCP Server)** with a future path to Approach B if needed.

### Rationale

1. **MCP is already a first-class pattern in ironclaw.** The infrastructure exists — `mcp/client.rs`, `mcp/transport.rs`, `mcp/lifecycle.rs`, `mcp/registry.rs`. Adding a browser MCP server is a configuration change, not a code change. This aligns with the design philosophy of "start from what works."

2. **The accessibility tree approach is the right output format for LLMs.** Microsoft has invested significant engineering in making Playwright MCP's output LLM-friendly. Building equivalent quality in a native Rust tool would be substantial effort for the same result. This follows "put the right work in the right place."

3. **Zero implementation risk.** No new Rust code to write, test, or maintain for browser logic. The browser automation ecosystem is complex and fast-moving — let Microsoft handle it.

4. **Per-project scoping works naturally.** Projects that need browser automation add it to their `PROJECT.md` frontmatter. Projects that don't, don't pay any cost. This matches the progressive disclosure model.

5. **The Node.js dependency is acceptable.** Users who need browser automation almost certainly already have Node.js installed (it's the runtime for most web development tooling). The `npx` command auto-installs the package. For users who don't want Node.js, browser automation simply isn't configured — no impact on the core binary.

### What this looks like in practice

No code changes to ironclaw. A user or the agent adds browser automation to a project:

```yaml
# PROJECT.md frontmatter
mcp_servers:
  - name: playwright
    command: "npx"
    args: ["@playwright/mcp@latest", "--headless"]
```

Or globally in `config.toml`:

```toml
[mcp.servers.playwright]
command = "npx"
args = ["@playwright/mcp@latest", "--headless"]
```

The agent then has access to all Playwright MCP tools (`browser_navigate`, `browser_snapshot`, `browser_click`, etc.) while that project is active.

### Future path to native tool

If the Node.js dependency becomes unacceptable, or if a lightweight subset of browser capabilities is needed for pulse tasks (where spawning an MCP server on every heartbeat is wasteful), then a native tool using `chromey` can be added later:

1. Implement a minimal `BrowserTool` in `src/tools/browser.rs` using `chromey`.
2. Focus on three operations: navigate + extract (accessibility tree via CDP), screenshot, and JS execution.
3. Use CDP's `Accessibility.getFullAXTree` for LLM-friendly output.
4. Gate it like `exec` via `ToolFilter`.
5. Keep Playwright MCP available for advanced automation that exceeds the native tool's capabilities.

This path is additive — the MCP approach doesn't prevent adding a native tool later. But the MCP approach is available today with no implementation cost.

### Bundled skill as documentation

Regardless of approach, a bundled skill (`skills/browser-automation/SKILL.md`) should document how to use browser automation effectively. This gives the agent instructions for common patterns (navigating, extracting data, taking screenshots, multi-step workflows) without needing to figure it out from tool definitions alone.

---

## Appendix: Rust CDP Crate Details

### chromiumoxide

- **Crate**: `chromiumoxide` v0.7.0
- **GitHub**: github.com/mattsse/chromiumoxide
- **Runtime**: `async-std` default, `tokio` via feature flag
- **Status**: Slowing development, multiple forks have appeared

### chromey

- **Crate**: `chromey` v2.37.158
- **GitHub**: Fork from Spider project
- **Runtime**: Tokio-native (`tokio-tungstenite`)
- **Status**: Very active, frequent releases
- **Note**: Best choice if going native CDP. Inherits chromiumoxide API with tokio focus and active maintenance.

### headless_chrome

- **Crate**: `headless_chrome` v1.0.17
- **GitHub**: github.com/rust-headless-chrome/rust-headless-chrome
- **Runtime**: Synchronous (plain threads)
- **Status**: Active releases through 2025
- **Note**: Disqualified for ironclaw due to synchronous API.

### fantoccini

- **Crate**: `fantoccini` v0.22.0
- **GitHub**: github.com/jonhoo/fantoccini (1900+ stars)
- **Runtime**: Tokio-native
- **Status**: Very active, maintained by Jon Gjengset
- **Note**: WebDriver-based (not CDP). Requires external driver process. More portable but less capable than CDP.

### Playwright MCP

- **Package**: `@playwright/mcp` (npm)
- **GitHub**: github.com/microsoft/playwright-mcp
- **Status**: Microsoft-maintained, weekly updates, released March 2025
- **Protocol**: MCP over stdio (JSON-RPC 2.0)
- **Note**: Recommended approach. Uses accessibility tree for LLM-friendly output.
