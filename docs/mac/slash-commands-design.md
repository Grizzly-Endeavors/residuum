# Residuum Chat — Slash Commands Design Spec

**Date:** 2026-03-28
**Status:** Approved

---

## Overview

Add slash command support to the Residuum Chat Mac app, consistent with the web UI. Typing `/` as the first character in the input bar shows a filtered autocomplete menu above the input row. Selecting a command either executes it immediately or (for `/inbox`) populates the input for argument entry.

---

## New Files

| File | Responsibility |
|---|---|
| `mac/Residuum/Agents/SlashCommand.swift` | `SlashCommand` model + `COMMAND_REGISTRY` |
| `mac/Residuum/Views/CommandMenu.swift` | Menu view rendered above the input row |

## Modified Files

| File | Change |
|---|---|
| `mac/Residuum/Views/InputBar.swift` | Detect `/`, show/hide menu, handle selection + `/inbox` prefix dispatch |
| `mac/Residuum/Agents/AgentStore.swift` | Add `appendSystemMessage(_ content: String)` + `appendSystemBlock(_ content: String)` |
| `mac/Residuum/Agents/AgentTab.swift` | Add `verboseEnabled: Bool` (default `false`) |
| `mac/Residuum/Agents/Message.swift` | Add `.systemBlock` role variant to `ChatMessage.Role` |
| `mac/Residuum/Views/MessageRow.swift` | Render `.systemBlock` as blue-bordered monospace block |

---

## Data Model

### `SlashCommand`

```swift
struct SlashCommand {
    let name: String        // e.g. "/observe"
    let description: String // e.g. "Trigger memory observation"
    let hasArgs: Bool       // only true for /inbox
}
```

### `COMMAND_REGISTRY`

Ordered array of all eight commands:

| name | description | hasArgs |
|---|---|---|
| `/help` | Show this help message | false |
| `/verbose` | Toggle tool call visibility | false |
| `/status` | Show connection status | false |
| `/observe` | Trigger memory observation | false |
| `/reflect` | Trigger memory reflection | false |
| `/context` | Show current project context | false |
| `/reload` | Reload gateway configuration | false |
| `/inbox` | Add a message to the inbox | true |

---

## Message Model Changes

`ChatMessage.Role` gains a new case:

```swift
case systemBlock  // blue-bordered monospace block for structured output
```

`system` (centred italic) remains for simple one-line notices.
`systemBlock` is used for `/help`, `/status`, and any structured text output.

---

## AgentStore Changes

Two new public methods:

```swift
/// Appends a centred italic system notice to the selected tab's feed.
func appendSystemMessage(_ content: String)

/// Appends a blue-bordered monospace block to the selected tab's feed.
func appendSystemBlock(_ content: String)
```

Both append to `tabs[selectedTabIndex].messages` on the main queue.

---

## AgentTab Changes

```swift
var verboseEnabled: Bool = false
```

Tracks per-tab verbose state. On connect (`didOpenWithProtocol`), `ResiduumConnection` should send `SetVerbose(enabled: verboseEnabled)` — this replaces the current hardcoded `setVerbose(enabled: true)`.

---

## InputBar Changes

Three new `@State` properties:

```swift
@State private var showMenu = false
@State private var menuQuery = ""       // characters after the /
@State private var menuIndex = 0        // selected row index
```

### Show/hide logic

In `onChange(of: text)` (or on the text field's `onChange`):
- If `text` starts with `/` and contains no space → `showMenu = true`, `menuQuery = text.dropFirst()`
- Otherwise → `showMenu = false`

### Keyboard handling

When `showMenu` is true:
- `↑` / `↓` → move `menuIndex`
- `Return` / `Tab` → call `handleCommandSelect(filtered[menuIndex])`
- `Escape` → `showMenu = false`

### Command selection

```swift
func handleCommandSelect(_ cmd: SlashCommand) {
    if cmd.hasArgs {
        // /inbox: populate input, let user type the argument
        text = cmd.name + " "
        showMenu = false
    } else {
        text = ""
        showMenu = false
        executeCommand(cmd)
    }
}
```

### `/inbox` prefix in sendMessage

```swift
private func sendMessage() {
    let content = text.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !content.isEmpty else { return }
    if content.hasPrefix("/inbox ") {
        let body = String(content.dropFirst("/inbox ".count))
        guard !body.isEmpty else { return }
        store.selectedTab?.connection.send(.inboxAdd(body: body))
        text = ""
        attachedImages = []
        return
    }
    // existing send path...
}
```

### Filtered commands

```swift
var filteredCommands: [SlashCommand] {
    if menuQuery.isEmpty { return COMMAND_REGISTRY }
    return COMMAND_REGISTRY.filter { $0.name.hasPrefix("/" + menuQuery) }
}
```

---

## CommandMenu View

Rendered above the input row inside `InputBar.body` when `showMenu && !filteredCommands.isEmpty`:

```
VStack(spacing: 0) {
    VeinDivider()
    ForEach(filteredCommands) { cmd in
        CommandMenuItem(cmd, isSelected: index == menuIndex)
            .onTapGesture { handleCommandSelect(cmd) }
    }
    VeinDivider()
}
```

Each `CommandMenuItem`:
- Left: command name in `Style.mono(size: 11)` — `Style.blue` when selected, `Style.textMuted` otherwise
- Right: description in `Style.literata(size: 11)` italic — `Style.textMuted` when selected, `Style.textDim` otherwise
- Background: `Style.surfaceRaised` when selected, transparent otherwise
- Padding: 6pt vertical, 14pt horizontal

---

## Command Execution

### `/help`
```
store.appendSystemBlock("""
/help        Show this help message
/verbose     Toggle tool call visibility
/status      Show connection status
/observe     Trigger memory observation
/reflect     Trigger memory reflection
/context     Show current project context
/reload      Reload gateway configuration
/inbox       Add a message to the inbox
""")
```

### `/verbose`
```swift
guard let idx = store.selectedTabIndex else { return }
store.tabs[idx].verboseEnabled.toggle()
let enabled = store.tabs[idx].verboseEnabled
store.tabs[idx].connection.send(.setVerbose(enabled: enabled))
store.appendSystemMessage("Verbose mode \(enabled ? "enabled" : "disabled").")
```

### `/status`
```swift
let tab = store.selectedTab
let state = tab?.connection.state ?? .disconnected
let stateStr = state == .connected ? "connected" : state == .connecting ? "connecting…" : "disconnected"
let verbose = tab?.verboseEnabled == true ? "on" : "off"
store.appendSystemBlock("""
agent    \(tab?.name ?? "Default") · port \(tab?.port ?? 7700)
status   \(stateStr)
verbose  \(verbose)
""")
```

### `/observe`, `/reflect`, `/context`
```swift
store.selectedTab?.connection.send(.serverCommand(name: "observe", args: nil))
// (replace "observe" with the relevant command name)
```

### `/reload`
```swift
store.selectedTab?.connection.send(.reload)
```

---

## MessageRow Changes

`MessageRow` routes `.systemBlock` to a new `SystemBlock` private view:

```swift
case .systemBlock: SystemBlock(content: message.content)
```

`SystemBlock` renders as a horizontal `HStack`:
- A 3pt wide `Rectangle` filled with `Style.blue.opacity(0.2)` on the left (rounded)
- `Text(content)` in `Style.mono(size: 11)`, `Style.textMuted`, leading-aligned

---

## ResiduumConnection Change

`didOpenWithProtocol` currently hardcodes `setVerbose(enabled: true)`. Change to `setVerbose(enabled: false)` — verbose defaults off, matching the web UI default.

The `/verbose` command sends `SetVerbose` directly to the live connection without reconnecting. No init parameter change is needed.

---

## Out of Scope

- Persisting verbose state across app restarts
- Adding new slash commands beyond the 8 listed
- Command history / recents
