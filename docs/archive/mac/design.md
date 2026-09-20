# Residuum for Mac — Design Spec

**Date:** 2026-03-28
**Status:** Approved

---

## Overview

A native SwiftUI Mac app that serves as a first-class client for the Residuum daemon. It connects to the locally-running Rust daemon via WebSocket and provides a chat interface for interacting with personal AI agents — with file upload support, agent tab switching, and a settings panel. The app lives entirely in the menu bar with an optional expanded window mode.

The app lives at `residuum/mac/` within the existing monorepo. It is a pure client — it does not start, stop, or manage the daemon.

---

## Architecture

### App Mode

**Hybrid menu bar + optional window.** The app has no Dock icon. It lives as an `NSStatusItem` in the menu bar. Clicking the icon opens an `NSPopover` (420 × 520pt). A persistent "open in window" button at the bottom of the popover expands to a full detached `NSWindow` that can float above other apps.

Implementation uses `NSStatusItem`, `NSPopover`, and `NSWindow` directly — not SwiftUI's `MenuBarExtra`, which lacks the sizing control needed for the expand-to-window transition.

### Data Flow

```
AgentStore (@Observable)
    ├── ResiduumConnection → ws://127.0.0.1:7700  (default agent)
    ├── ResiduumConnection → ws://127.0.0.1:7701  (named agent "Aria")
    └── ResiduumConnection → ws://127.0.0.1:7702  (named agent "Sentinel")
         ↓  each publishes ServerMessage
AgentStore routes to per-agent [Message] arrays
    ↓  @Environment
SwiftUI Views
```

Each agent in Residuum runs as a **separate daemon on a separate port**. The default agent uses port 7700; named agents start at 7701 and are registered in `~/.residuum/agent_registry/registry.toml`.

`AgentStore` reads the registry on launch to discover named agents, then creates one `ResiduumConnection` per agent. Each connection is independent — switching tabs switches the active connection. `AgentStore` is injected into the view hierarchy via `@Environment`. Views observe `AgentStore` directly — no Combine, no separate ViewModels layer.

### Project Structure

```
mac/
├── Residuum.xcodeproj
└── Residuum/
    ├── App/
    │   ├── ResiduumApp.swift        # @main, NSStatusItem + NSPopover setup
    │   └── AppDelegate.swift        # Popover and window lifecycle
    ├── Connection/
    │   ├── ResiduumConnection.swift  # WebSocket via URLSessionWebSocketTask
    │   └── Protocol.swift           # ClientMessage / ServerMessage Codable enums
    ├── Agents/
    │   └── AgentStore.swift         # @Observable, per-agent message history
    ├── Views/
    │   ├── PopoverView.swift         # Root view rendered inside NSPopover
    │   ├── ChatView.swift            # Scrollable message list
    │   ├── MessageRow.swift          # Renders user / assistant / system messages
    │   ├── ToolGroup.swift           # Collapsible tool call visualization
    │   ├── InputBar.swift            # Text field, file attach, send button
    │   ├── TabBar.swift              # Pill-style agent tab bar
    │   └── SettingsView.swift        # Settings sheet
    └── Resources/
        ├── Assets.xcassets           # Menu bar icon (template image)
        └── Fonts/                    # Cinzel, Literata, JetBrains Mono
```

---

## WebSocket Connection

### Endpoint

`ws://<host>:<port>/ws` — one connection per agent. The host is configurable in Settings (default `127.0.0.1`). Ports come from the agent registry: `7700` for the default agent, registry-defined ports for named agents. Only the host is a user setting; ports are owned by the registry.

### Lifecycle

- Connects on app launch
- Sends `set_verbose: { enabled: true }` immediately after connect so tool call events stream through
- Sends `ping` every 30 seconds to keep the connection alive
- On disconnect: retries with exponential backoff — 1s, 2s, 4s, … capped at 30s
- Publishes a `connectionState` enum: `.connecting`, `.connected`, `.disconnected`

### Protocol Types (`Protocol.swift`)

Mirrors the Rust types exactly. Both enums use `snake_case` JSON tag names via `CodingKeys`.

**ClientMessage** (sent to daemon):
- `send_message` — `id: String`, `content: String`, `images: [ImageData]`
- `set_verbose` — `enabled: Bool`
- `ping`
- `reload`
- `server_command` — `name: String`, `args: String?`
- `inbox_add` — `body: String`

**ServerMessage** (received from daemon):
- `turn_started` — `reply_to: String`
- `tool_call` — `id: String`, `name: String`, `arguments: JSON`
- `tool_result` — `tool_call_id: String`, `name: String`, `output: String`, `is_error: Bool`
- `response` — `reply_to: String`, `content: String`
- `system_event` — `source: String`, `content: String`
- `broadcast_response` — `content: String`
- `error` — `reply_to: String?`, `message: String`
- `notice` — `message: String`
- `pong`

---

## UI

### Aesthetic

Matches the existing Residuum web UI exactly:

| Element | Value |
|---|---|
| Background | `#0e0e10` |
| Surface | `#111114` / `#1a1a1d` |
| Border | `#1e1e22` / `#222226` |
| Blue accent (vein) | `#3b8bdb` |
| Moss green | `#6b7a4a` |
| Body text | `#c0c0c0` |
| Muted text | `#555` |
| Heading font | Cinzel (serif, letter-spaced) |
| Body font | Literata 300 weight |
| Code / labels font | JetBrains Mono |

Horizontal blue vein dividers (`linear-gradient` from transparent → `#3b8bdb44` → transparent) separate major layout zones.

### Header

Left: `RESIDUUM` wordmark in Cinzel, blue, letter-spaced.
Centre: Pill-style agent tab bar. Active tab has a `#1e1e22` background and border. Inactive tabs are muted.
Right: ⚙ settings button.

### Agent Tabs

Each tab corresponds to one running agent daemon. `AgentStore` discovers agents by reading `~/.residuum/agent_registry/registry.toml` (a TOML file with a `[[agents]]` list of `name` + `port` entries). The default agent (port 7700, no name) is always shown first as "Default". Named agents appear as additional tabs in registry order.

If the registry file is absent or empty, only the "Default" tab is shown. If a specific agent's daemon is not running, that tab's connection shows as disconnected independently of the others.

### Chat Feed

Scrollable list of messages rendered by `MessageRow`:

- **User messages** — right-aligned bubble, `#1a2535` background, blue-tinted border, rounded with a small tail bottom-right
- **Assistant messages** — left-aligned, no bubble background, `Literata` body text
- **Tool groups** — collapsible `ToolGroup` showing tool name in moss green; expanded view shows each tool call/result in a `JetBrains Mono` block with a blue left border
- **Thinking indicator** — three animated dots when a turn is in progress
- **System events / notices** — centred, muted, italic

### Input Bar

Bottom of the popover/window, separated from the chat feed by a vein divider.

- File attachment chips appear above the input row when files are selected (filename + ✕ to remove)
- Input row: paperclip button → `NSOpenPanel` → `text field` → send button (filled blue circle with ↑)
- Send is disabled while disconnected or while a turn is in progress

### Disconnected State

When the daemon is not reachable:
- Menu bar icon dims (template image at reduced opacity)
- Popover shows a centred message: daemon name, `residuum serve` hint, and a `Reconnect` button
- No alerts, no repeated popups

### Expand to Window

A small `⤢ open in window` text button at the bottom-right of the popover. Clicking it:
1. Closes the popover
2. Opens an `NSWindow` (800 × 600pt, resizable, no Dock icon) with the same `PopoverView` content

---

## File Uploads

- Trigger: paperclip button opens `NSOpenPanel`
- v1 scope: images only (PNG, JPEG, GIF, WEBP) — matching the daemon's `ImageData` type
- Selected files shown as chips in the input area; each chip has a remove button
- On send: files are base64-encoded and included in the `images` array of `send_message`
- Other file types (PDF, text, etc.) are out of scope for v1

---

## Settings

Opens as a sheet over the popover / panel in the expanded window.

**Connection section:**
- Host field (default: `127.0.0.1`)
- Port field (default: `7700`)
- Connection status indicator (connected / connecting / disconnected)

Settings persist in `UserDefaults`. The app reconnects automatically when host/port change.

No other settings for v1. The daemon owns its own configuration.

---

## Out of Scope (v1)

- Starting or stopping the daemon
- Setup / onboarding flow (use the web UI for first-time config)
- Non-image file types in uploads
- Notifications / background running
- Multiple simultaneous windows
- macOS Sonoma+ features (e.g., interactive widgets)
