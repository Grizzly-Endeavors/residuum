# Slash Commands Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add slash command autocomplete to Residuum Chat — type `/` in the input bar to see a filtered menu of 8 commands that execute immediately or populate the input for argument entry.

**Architecture:** `SlashCommand` model + `COMMAND_REGISTRY` drive a `CommandMenu` view rendered above the input row inside `InputBar`. Three new `@State` vars in `InputBar` handle show/hide/filter/selection. Command execution calls `AgentStore` helpers or `ResiduumConnection.send` directly. Client-side commands (`/help`, `/status`, `/verbose`) append styled system messages to the chat feed.

**Tech Stack:** Swift 5.9+, SwiftUI, `@Observable`, XCTest

**Spec:** `docs/mac/slash-commands-design.md`

---

## File Map

```
mac/Residuum/Agents/
  SlashCommand.swift          ← NEW: SlashCommand model + COMMAND_REGISTRY
  AgentTab.swift              ← ADD: verboseEnabled: Bool
  AgentStore.swift            ← ADD: appendSystemMessage, appendSystemBlock
  Message.swift               ← ADD: .systemBlock role case

mac/Residuum/Views/
  CommandMenu.swift           ← NEW: CommandMenuItem + CommandMenu views
  InputBar.swift              ← MODIFY: slash detection, menu, keyboard nav, execution
  MessageRow.swift            ← ADD: SystemBlock private view, route .systemBlock

mac/Residuum/Connection/
  ResiduumConnection.swift    ← CHANGE: setVerbose default false → false (was true)

mac/ResiduumTests/
  SlashCommandTests.swift     ← NEW: registry + AgentStore helper tests
```

---

## Task 1: `.systemBlock` message role + SystemBlock view

**Files:**
- Modify: `mac/Residuum/Agents/Message.swift`
- Modify: `mac/Residuum/Views/MessageRow.swift`

- [ ] **Step 1: Add `.systemBlock` to `ChatMessage.Role`**

  In `mac/Residuum/Agents/Message.swift`, replace the `Role` enum:

  ```swift
  enum Role {
      case user
      case assistant
      case system       // centred italic — simple one-line notices
      case systemBlock  // blue-bordered monospace block — structured output (/help, /status)
  }
  ```

- [ ] **Step 2: Add `SystemBlock` view and route it in `MessageRow`**

  In `mac/Residuum/Views/MessageRow.swift`, replace the `body` switch and add the new private view:

  ```swift
  var body: some View {
      switch message.role {
      case .user:        UserBubble(content: message.content)
      case .assistant:   AssistantMessage(message: message)
      case .system:      SystemNotice(content: message.content)
      case .systemBlock: SystemBlock(content: message.content)
      }
  }
  ```

  Add this private struct after `SystemNotice`:

  ```swift
  // MARK: - System block (blue-bordered monospace — /help, /status output)

  private struct SystemBlock: View {
      let content: String

      var body: some View {
          HStack(alignment: .top, spacing: 10) {
              Rectangle()
                  .fill(Style.blue.opacity(0.2))
                  .frame(width: 3)
                  .clipShape(RoundedRectangle(cornerRadius: 2))
              Text(content)
                  .font(Style.mono(size: 11))
                  .foregroundStyle(Style.textMuted)
                  .frame(maxWidth: .infinity, alignment: .leading)
                  .textSelection(.enabled)
          }
      }
  }
  ```

- [ ] **Step 3: Build**

  ```bash
  cd /Users/jflinn/projects/residuum/mac
  xcodegen generate
  xcodebuild -project ResiduumChat.xcodeproj -scheme ResiduumChat \
    -configuration Debug build 2>&1 | tail -3
  ```

  Expected: `** BUILD SUCCEEDED **`

- [ ] **Step 4: Commit**

  ```bash
  cd /Users/jflinn/projects/residuum
  git add mac/Residuum/Agents/Message.swift mac/Residuum/Views/MessageRow.swift \
          mac/ResiduumChat.xcodeproj/
  git commit -m "feat(mac): add systemBlock message role and SystemBlock view"
  ```

---

## Task 2: `SlashCommand` model, registry, and tests

**Files:**
- Create: `mac/Residuum/Agents/SlashCommand.swift`
- Create: `mac/ResiduumTests/SlashCommandTests.swift`

- [ ] **Step 1: Write the failing tests first**

  Create `mac/ResiduumTests/SlashCommandTests.swift`:

  ```swift
  import XCTest
  @testable import ResiduumChat

  final class SlashCommandTests: XCTestCase {

      // MARK: - Registry shape

      func testRegistryHasEightCommands() {
          XCTAssertEqual(COMMAND_REGISTRY.count, 8)
      }

      func testOnlyInboxHasArgs() {
          let withArgs = COMMAND_REGISTRY.filter { $0.hasArgs }
          XCTAssertEqual(withArgs.count, 1)
          XCTAssertEqual(withArgs.first?.name, "/inbox")
      }

      func testAllCommandsStartWithSlash() {
          for cmd in COMMAND_REGISTRY {
              XCTAssertTrue(cmd.name.hasPrefix("/"),
                  "\(cmd.name) must start with /")
          }
      }

      func testRegistryContainsExpectedCommands() {
          let names = COMMAND_REGISTRY.map { $0.name }
          XCTAssertEqual(names, [
              "/help", "/verbose", "/status",
              "/observe", "/reflect", "/context",
              "/reload", "/inbox"
          ])
      }

      // MARK: - Filtering

      func testEmptyQueryReturnsAll() {
          let filtered = COMMAND_REGISTRY.filter {
              "".isEmpty || $0.name.hasPrefix("/" + "")
          }
          XCTAssertEqual(filtered.count, 8)
      }

      func testPrefixFilterMatchesSingle() {
          let filtered = COMMAND_REGISTRY.filter { $0.name.hasPrefix("/ob") }
          XCTAssertEqual(filtered.count, 1)
          XCTAssertEqual(filtered.first?.name, "/observe")
      }

      func testPrefixFilterMatchesMultiple() {
          let filtered = COMMAND_REGISTRY.filter { $0.name.hasPrefix("/re") }
          XCTAssertEqual(filtered.count, 2) // /reflect, /reload
          XCTAssertEqual(filtered.map { $0.name }, ["/reflect", "/reload"])
      }

      func testPrefixFilterNoMatch() {
          let filtered = COMMAND_REGISTRY.filter { $0.name.hasPrefix("/zzz") }
          XCTAssertTrue(filtered.isEmpty)
      }
  }
  ```

- [ ] **Step 2: Run tests — expect build failure**

  ```bash
  cd /Users/jflinn/projects/residuum/mac
  xcodegen generate
  xcodebuild test -project ResiduumChat.xcodeproj -scheme ResiduumChat \
    -destination 'platform=macOS' 2>&1 | grep -E "error:|FAILED" | head -5
  ```

  Expected: build errors — `COMMAND_REGISTRY` not defined.

  > **Note for new Swift developers:** If you see `@testable import ResiduumChat` fail to compile with "no such module", try `@testable import Residuum_Chat` instead — the module name is derived from the target name with spaces replaced by underscores.

- [ ] **Step 3: Create `SlashCommand.swift`**

  Create `mac/Residuum/Agents/SlashCommand.swift`:

  ```swift
  import Foundation

  /// A single slash command available in the input bar.
  struct SlashCommand: Identifiable {
      var id: String { name }
      /// The command name including the leading slash, e.g. `"/observe"`.
      let name: String
      /// Short description shown in the autocomplete menu.
      let description: String
      /// True only for commands that take an argument (currently only `/inbox`).
      let hasArgs: Bool
  }

  /// All available slash commands, in display order.
  let COMMAND_REGISTRY: [SlashCommand] = [
      SlashCommand(name: "/help",    description: "Show this help message",      hasArgs: false),
      SlashCommand(name: "/verbose", description: "Toggle tool call visibility", hasArgs: false),
      SlashCommand(name: "/status",  description: "Show connection status",      hasArgs: false),
      SlashCommand(name: "/observe", description: "Trigger memory observation",  hasArgs: false),
      SlashCommand(name: "/reflect", description: "Trigger memory reflection",   hasArgs: false),
      SlashCommand(name: "/context", description: "Show current project context",hasArgs: false),
      SlashCommand(name: "/reload",  description: "Reload gateway configuration",hasArgs: false),
      SlashCommand(name: "/inbox",   description: "Add a message to the inbox",  hasArgs: true),
  ]
  ```

- [ ] **Step 4: Run tests — expect passing**

  ```bash
  cd /Users/jflinn/projects/residuum/mac
  xcodegen generate
  xcodebuild test -project ResiduumChat.xcodeproj -scheme ResiduumChat \
    -destination 'platform=macOS' 2>&1 | grep -E "Test Suite|passed|failed" | tail -5
  ```

  Expected: `SlashCommandTests` suite passes (8 tests).

- [ ] **Step 5: Commit**

  ```bash
  cd /Users/jflinn/projects/residuum
  git add mac/Residuum/Agents/SlashCommand.swift \
          mac/ResiduumTests/SlashCommandTests.swift \
          mac/ResiduumChat.xcodeproj/
  git commit -m "feat(mac): add SlashCommand model and COMMAND_REGISTRY with tests"
  ```

---

## Task 3: `AgentStore` system message helpers + tests

**Files:**
- Modify: `mac/Residuum/Agents/AgentStore.swift`
- Modify: `mac/ResiduumTests/SlashCommandTests.swift`

- [ ] **Step 1: Add failing tests to `SlashCommandTests.swift`**

  Append these test methods to the `SlashCommandTests` class (inside the closing `}`):

  ```swift
  // MARK: - AgentStore system messages

  func testAppendSystemMessageAddsToSelectedTab() {
      let store = AgentStore()
      let initial = store.selectedTab?.messages.count ?? 0
      store.appendSystemMessage("hello notice")
      XCTAssertEqual(store.selectedTab?.messages.count, initial + 1)
      let last = store.selectedTab?.messages.last
      XCTAssertEqual(last?.content, "hello notice")
      XCTAssertEqual(last?.role, .system)
  }

  func testAppendSystemBlockAddsSystemBlockRole() {
      let store = AgentStore()
      store.appendSystemBlock("key   value")
      let last = store.selectedTab?.messages.last
      XCTAssertEqual(last?.role, .systemBlock)
      XCTAssertEqual(last?.content, "key   value")
  }

  func testAppendDoesNothingWhenNoTabsSelected() {
      let store = AgentStore()
      store.tabs.removeAll()
      // Must not crash
      store.appendSystemMessage("orphan")
      store.appendSystemBlock("orphan block")
      // No assertion needed — absence of crash is the test
  }
  ```

- [ ] **Step 2: Run tests — expect failure**

  ```bash
  cd /Users/jflinn/projects/residuum/mac
  xcodebuild test -project ResiduumChat.xcodeproj -scheme ResiduumChat \
    -destination 'platform=macOS' 2>&1 | grep -E "error:|FAILED" | head -5
  ```

  Expected: build errors — `appendSystemMessage` and `appendSystemBlock` not defined on `AgentStore`.

- [ ] **Step 3: Add the helpers to `AgentStore.swift`**

  In `mac/Residuum/Agents/AgentStore.swift`, add these two methods to the Public API section (after `reconnectAll(host:)`):

  ```swift
  /// Appends a centred italic system notice to the selected tab's feed.
  func appendSystemMessage(_ content: String) {
      guard let idx = selectedTabIndex else { return }
      tabs[idx].messages.append(ChatMessage(role: .system, content: content))
  }

  /// Appends a blue-bordered monospace block to the selected tab's feed.
  func appendSystemBlock(_ content: String) {
      guard let idx = selectedTabIndex else { return }
      tabs[idx].messages.append(ChatMessage(role: .systemBlock, content: content))
  }
  ```

- [ ] **Step 4: Run tests — expect passing**

  ```bash
  cd /Users/jflinn/projects/residuum/mac
  xcodebuild test -project ResiduumChat.xcodeproj -scheme ResiduumChat \
    -destination 'platform=macOS' 2>&1 | grep -E "Test Suite|passed|failed" | tail -5
  ```

  Expected: all tests pass.

- [ ] **Step 5: Commit**

  ```bash
  cd /Users/jflinn/projects/residuum
  git add mac/Residuum/Agents/AgentStore.swift \
          mac/ResiduumTests/SlashCommandTests.swift
  git commit -m "feat(mac): add appendSystemMessage and appendSystemBlock to AgentStore"
  ```

---

## Task 4: `AgentTab.verboseEnabled` + fix default verbose

**Files:**
- Modify: `mac/Residuum/Agents/AgentTab.swift`
- Modify: `mac/Residuum/Connection/ResiduumConnection.swift`

- [ ] **Step 1: Add `verboseEnabled` to `AgentTab`**

  In `mac/Residuum/Agents/AgentTab.swift`, add one property after `pendingCorrelationId`:

  ```swift
  /// Whether tool calls and results are shown for this agent's feed.
  /// Toggled by the /verbose command.
  var verboseEnabled: Bool = false
  ```

  The full struct now looks like:

  ```swift
  import Foundation

  /// One tab in the UI, corresponding to one Residuum agent daemon.
  struct AgentTab: Identifiable {
      let id: UUID
      /// Display name — "Default" for the default agent, agent name for named agents.
      let name: String
      let port: UInt16
      /// The live WebSocket connection to this agent's daemon.
      var connection: ResiduumConnection
      /// All messages in this agent's conversation (chronological order).
      var messages: [ChatMessage]
      /// True while the agent is processing a turn (between turn_started and response).
      var isThinking: Bool
      /// The pending tool group being accumulated during the current turn.
      var pendingToolCalls: [ToolCallData]
      /// Correlation ID of the in-flight turn, for matching response/error to turn.
      var pendingCorrelationId: String?
      /// Whether tool calls and results are shown for this agent's feed.
      var verboseEnabled: Bool = false

      init(name: String, port: UInt16, connection: ResiduumConnection) {
          self.id = UUID()
          self.name = name
          self.port = port
          self.connection = connection
          self.messages = []
          self.isThinking = false
          self.pendingToolCalls = []
      }
  }
  ```

- [ ] **Step 2: Fix `ResiduumConnection` verbose default**

  In `mac/Residuum/Connection/ResiduumConnection.swift`, find `didOpenWithProtocol` and change `enabled: true` to `enabled: false`:

  ```swift
  func urlSession(
      _ session: URLSession,
      webSocketTask: URLSessionWebSocketTask,
      didOpenWithProtocol protocol: String?
  ) {
      DispatchQueue.main.async {
          self.retryDelay = 1.0
          self.updateState(.connected)
          self.send(.setVerbose(enabled: false))  // ← was true, now false (default off)
          self.startPing()
      }
  }
  ```

- [ ] **Step 3: Build and run all tests**

  ```bash
  cd /Users/jflinn/projects/residuum/mac
  xcodebuild test -project ResiduumChat.xcodeproj -scheme ResiduumChat \
    -destination 'platform=macOS' 2>&1 | grep -E "Test Suite|passed|failed" | tail -5
  ```

  Expected: all tests pass.

- [ ] **Step 4: Commit**

  ```bash
  cd /Users/jflinn/projects/residuum
  git add mac/Residuum/Agents/AgentTab.swift \
          mac/Residuum/Connection/ResiduumConnection.swift
  git commit -m "feat(mac): add verboseEnabled to AgentTab, default verbose to off"
  ```

---

## Task 5: `CommandMenu` view

**Files:**
- Create: `mac/Residuum/Views/CommandMenu.swift`

- [ ] **Step 1: Create `CommandMenu.swift`**

  Create `mac/Residuum/Views/CommandMenu.swift`:

  ```swift
  import SwiftUI

  /// The autocomplete menu shown above the input bar when the user types `/`.
  ///
  /// Rendered as a list of `CommandMenuItem` rows separated by vein dividers.
  /// The parent (`InputBar`) owns the selection index and calls `onSelect` on tap or keyboard Enter.
  struct CommandMenu: View {
      let commands: [SlashCommand]
      let selectedIndex: Int
      let onSelect: (SlashCommand) -> Void

      var body: some View {
          VStack(spacing: 0) {
              VeinDivider()
              ForEach(Array(commands.enumerated()), id: \.element.id) { index, cmd in
                  CommandMenuItem(
                      command: cmd,
                      isSelected: index == selectedIndex
                  )
                  .onTapGesture { onSelect(cmd) }
              }
              VeinDivider()
          }
          .background(Style.background)
      }
  }

  /// A single row in the command menu.
  private struct CommandMenuItem: View {
      let command: SlashCommand
      let isSelected: Bool

      var body: some View {
          HStack(spacing: 0) {
              Text(command.name)
                  .font(Style.mono(size: 11))
                  .foregroundStyle(isSelected ? Style.blue : Style.textMuted)
                  .frame(width: 90, alignment: .leading)
              Text(command.description)
                  .font(Style.literata(size: 11))
                  .italic()
                  .foregroundStyle(isSelected ? Style.textMuted : Style.textDim)
              Spacer()
          }
          .padding(.horizontal, 14)
          .padding(.vertical, 6)
          .background(isSelected ? Style.surfaceRaised : Color.clear)
      }
  }
  ```

- [ ] **Step 2: Build**

  ```bash
  cd /Users/jflinn/projects/residuum/mac
  xcodegen generate
  xcodebuild -project ResiduumChat.xcodeproj -scheme ResiduumChat \
    -configuration Debug build 2>&1 | tail -3
  ```

  Expected: `** BUILD SUCCEEDED **`

- [ ] **Step 3: Commit**

  ```bash
  cd /Users/jflinn/projects/residuum
  git add mac/Residuum/Views/CommandMenu.swift mac/ResiduumChat.xcodeproj/
  git commit -m "feat(mac): add CommandMenu and CommandMenuItem views"
  ```

---

## Task 6: Wire slash commands into `InputBar`

**Files:**
- Modify: `mac/Residuum/Views/InputBar.swift`

This is the largest single task. Replace the entire file with the version below, which adds menu state, text detection, keyboard handling, and all eight command executions.

- [ ] **Step 1: Replace `InputBar.swift`**

  Replace `mac/Residuum/Views/InputBar.swift` entirely:

  ```swift
  import SwiftUI
  import AppKit

  /// Text input, file attachment chips, send button, and slash command autocomplete.
  struct InputBar: View {
      @Environment(AgentStore.self) private var store
      @State private var text = ""
      @State private var attachedImages: [AttachedImage] = []
      @FocusState private var focused: Bool

      // MARK: - Command menu state
      @State private var showMenu = false
      @State private var menuQuery = ""   // characters typed after the /
      @State private var menuIndex = 0    // currently highlighted row

      private var filteredCommands: [SlashCommand] {
          if menuQuery.isEmpty { return COMMAND_REGISTRY }
          return COMMAND_REGISTRY.filter { $0.name.hasPrefix("/" + menuQuery) }
      }

      private var canSend: Bool {
          let connected = store.selectedTab?.connection.state == .connected
          let notThinking = store.selectedTab?.isThinking == false
          let hasContent = !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
          return connected && notThinking && hasContent
      }

      var body: some View {
          VStack(spacing: 0) {
              // Command menu — appears above input when / is typed
              if showMenu && !filteredCommands.isEmpty {
                  CommandMenu(
                      commands: filteredCommands,
                      selectedIndex: min(menuIndex, filteredCommands.count - 1),
                      onSelect: handleCommandSelect
                  )
              }

              VStack(spacing: 8) {
                  // File chips
                  if !attachedImages.isEmpty {
                      ScrollView(.horizontal, showsIndicators: false) {
                          HStack(spacing: 6) {
                              ForEach(attachedImages) { img in
                                  FileChip(name: img.filename) {
                                      attachedImages.removeAll { $0.id == img.id }
                                  }
                              }
                          }
                          .padding(.horizontal, 2)
                      }
                  }

                  // Input row
                  HStack(spacing: 8) {
                      Button { pickFiles() } label: {
                          Image(systemName: "paperclip")
                              .font(.system(size: 14))
                              .foregroundStyle(Style.textMuted)
                      }
                      .buttonStyle(.plain)
                      .help("Attach an image")

                      TextField("", text: $text, axis: .vertical)
                          .font(Style.literata(size: 13))
                          .foregroundStyle(Style.textPrimary)
                          .textFieldStyle(.plain)
                          .lineLimit(1...6)
                          .focused($focused)
                          .onSubmit { handleReturn() }
                          .onChange(of: text) { _, newValue in updateMenu(for: newValue) }
                          .onKeyPress(.upArrow)   { moveMenu(by: -1) }
                          .onKeyPress(.downArrow) { moveMenu(by: 1) }
                          .onKeyPress(.escape)    { dismissMenu(); return .handled }
                          .placeholder(when: text.isEmpty) {
                              Text("Message \(store.selectedTab?.name ?? "agent")…")
                                  .font(Style.literata(size: 13))
                                  .foregroundStyle(Style.textMuted)
                          }

                      Button { sendMessage() } label: {
                          Image(systemName: "arrow.up")
                              .font(.system(size: 11, weight: .semibold))
                              .foregroundStyle(.white)
                              .frame(width: 24, height: 24)
                              .background(canSend ? Style.blue : Style.textDim)
                              .clipShape(Circle())
                      }
                      .buttonStyle(.plain)
                      .disabled(!canSend)
                  }
                  .padding(.horizontal, 10)
                  .padding(.vertical, 8)
                  .background(Style.surface)
                  .clipShape(RoundedRectangle(cornerRadius: 8))
                  .overlay(RoundedRectangle(cornerRadius: 8).stroke(Style.border, lineWidth: 1))
              }
              .padding(.horizontal, 12)
              .padding(.vertical, 10)
              .background(Style.background)
          }
      }

      // MARK: - Menu logic

      private func updateMenu(for value: String) {
          if value.hasPrefix("/"), !value.contains(" ") {
              menuQuery = String(value.dropFirst())
              menuIndex = 0
              showMenu = true
          } else {
              showMenu = false
          }
      }

      private func moveMenu(by delta: Int) -> KeyPress.Result {
          guard showMenu, !filteredCommands.isEmpty else { return .ignored }
          let count = filteredCommands.count
          menuIndex = (menuIndex + delta + count) % count
          return .handled
      }

      private func dismissMenu() {
          showMenu = false
      }

      private func handleReturn() {
          if showMenu, !filteredCommands.isEmpty {
              let idx = min(menuIndex, filteredCommands.count - 1)
              handleCommandSelect(filteredCommands[idx])
          } else if canSend {
              sendMessage()
          }
      }

      // MARK: - Command selection

      private func handleCommandSelect(_ cmd: SlashCommand) {
          if cmd.hasArgs {
              // For /inbox: populate field so user can type the argument
              text = cmd.name + " "
              showMenu = false
              focused = true
          } else {
              text = ""
              showMenu = false
              executeCommand(cmd)
          }
      }

      // MARK: - Command execution

      private func executeCommand(_ cmd: SlashCommand) {
          switch cmd.name {
          case "/help":
              store.appendSystemBlock(
                  "/help        Show this help message\n" +
                  "/verbose     Toggle tool call visibility\n" +
                  "/status      Show connection status\n" +
                  "/observe     Trigger memory observation\n" +
                  "/reflect     Trigger memory reflection\n" +
                  "/context     Show current project context\n" +
                  "/reload      Reload gateway configuration\n" +
                  "/inbox       Add a message to the inbox"
              )

          case "/verbose":
              guard let idx = store.selectedTabIndex else { return }
              store.tabs[idx].verboseEnabled.toggle()
              let enabled = store.tabs[idx].verboseEnabled
              store.tabs[idx].connection.send(.setVerbose(enabled: enabled))
              store.appendSystemMessage("Verbose mode \(enabled ? "enabled" : "disabled").")

          case "/status":
              let tab = store.selectedTab
              let stateStr: String
              switch tab?.connection.state ?? .disconnected {
              case .connected:    stateStr = "connected"
              case .connecting:   stateStr = "connecting…"
              case .disconnected: stateStr = "disconnected"
              }
              let verbose = tab?.verboseEnabled == true ? "on" : "off"
              store.appendSystemBlock(
                  "agent    \(tab?.name ?? "Default") · port \(tab?.port ?? 7700)\n" +
                  "status   \(stateStr)\n" +
                  "verbose  \(verbose)"
              )

          case "/observe":
              store.selectedTab?.connection.send(.serverCommand(name: "observe", args: nil))

          case "/reflect":
              store.selectedTab?.connection.send(.serverCommand(name: "reflect", args: nil))

          case "/context":
              store.selectedTab?.connection.send(.serverCommand(name: "context", args: nil))

          case "/reload":
              store.selectedTab?.connection.send(.reload)

          default:
              break
          }
      }

      // MARK: - Send

      private func sendMessage() {
          let content = text.trimmingCharacters(in: .whitespacesAndNewlines)
          guard !content.isEmpty else { return }

          // /inbox <body> — send as InboxAdd, not a regular message
          if content.hasPrefix("/inbox ") {
              let body = String(content.dropFirst("/inbox ".count))
                  .trimmingCharacters(in: .whitespacesAndNewlines)
              guard !body.isEmpty else { return }
              store.selectedTab?.connection.send(.inboxAdd(body: body))
              text = ""
              attachedImages = []
              return
          }

          let images = attachedImages.map { $0.imageData }
          store.sendMessage(content: content, images: images)
          text = ""
          attachedImages = []
      }

      // MARK: - File picker

      private func pickFiles() {
          assert(Thread.isMainThread, "NSOpenPanel must be presented on the main thread")
          let panel = NSOpenPanel()
          panel.allowsMultipleSelection = true
          panel.canChooseDirectories = false
          panel.allowedContentTypes = [.png, .jpeg, .gif, .webP, .bmp, .tiff]
          panel.message = "Choose images to attach"
          guard panel.runModal() == .OK else { return }
          for url in panel.urls {
              guard let data = try? Data(contentsOf: url),
                    let mediaType = mediaType(for: url) else { continue }
              let base64 = data.base64EncodedString()
              attachedImages.append(AttachedImage(
                  filename: url.lastPathComponent,
                  imageData: ImageData(mediaType: mediaType, data: base64)
              ))
          }
      }

      private func mediaType(for url: URL) -> String? {
          switch url.pathExtension.lowercased() {
          case "png":            return "image/png"
          case "jpg", "jpeg":    return "image/jpeg"
          case "gif":            return "image/gif"
          case "webp":           return "image/webp"
          case "bmp":            return "image/bmp"
          case "tiff", "tif":    return "image/tiff"
          default:               return nil
          }
      }
  }

  // MARK: - Supporting types

  private struct AttachedImage: Identifiable {
      let id = UUID()
      let filename: String
      let imageData: ImageData
  }

  private struct FileChip: View {
      let name: String
      let onRemove: () -> Void

      var body: some View {
          HStack(spacing: 5) {
              Image(systemName: "doc")
                  .font(.system(size: 10))
                  .foregroundStyle(Style.textMuted)
              Text(name)
                  .font(Style.mono(size: 10))
                  .foregroundStyle(Style.textMuted)
                  .lineLimit(1)
              Button(action: onRemove) {
                  Image(systemName: "xmark")
                      .font(.system(size: 9))
                      .foregroundStyle(Style.textDim)
              }
              .buttonStyle(.plain)
          }
          .padding(.horizontal, 8)
          .padding(.vertical, 4)
          .background(Style.surfaceRaised)
          .clipShape(RoundedRectangle(cornerRadius: 6))
          .overlay(RoundedRectangle(cornerRadius: 6).stroke(Style.border, lineWidth: 1))
      }
  }

  extension View {
      /// Overlays placeholder content when `condition` is true.
      func placeholder<Content: View>(
          when condition: Bool,
          @ViewBuilder content: () -> Content
      ) -> some View {
          overlay(content().allowsHitTesting(false).opacity(condition ? 1 : 0))
      }
  }
  ```

- [ ] **Step 2: Build**

  ```bash
  cd /Users/jflinn/projects/residuum/mac
  xcodebuild -project ResiduumChat.xcodeproj -scheme ResiduumChat \
    -configuration Debug build 2>&1 | tail -3
  ```

  Expected: `** BUILD SUCCEEDED **`

  **Common issues:**
  - `.onKeyPress` requires macOS 14+. If it fails, the deployment target in `project.yml` is already set to 14.0 so it should compile.
  - If `store.selectedTab?.connection.send(.reload)` gives a type error, the `ClientMessage.reload` case uses no associated value — check `Protocol.swift` line for the `.reload` case.

- [ ] **Step 3: Run all tests**

  ```bash
  cd /Users/jflinn/projects/residuum/mac
  xcodebuild test -project ResiduumChat.xcodeproj -scheme ResiduumChat \
    -destination 'platform=macOS' 2>&1 | grep -E "Test Suite|passed|failed" | tail -5
  ```

  Expected: all tests pass.

- [ ] **Step 4: Commit**

  ```bash
  cd /Users/jflinn/projects/residuum
  git add mac/Residuum/Views/InputBar.swift
  git commit -m "feat(mac): add slash command autocomplete and execution to InputBar"
  ```

---

## Task 7: Build release and install

- [ ] **Step 1: Build release**

  ```bash
  cd /Users/jflinn/projects/residuum/mac
  xcodebuild -project ResiduumChat.xcodeproj -scheme ResiduumChat \
    -configuration Release build \
    -derivedDataPath /tmp/residuum-chat-build 2>&1 | tail -3
  ```

  Expected: `** BUILD SUCCEEDED **`

- [ ] **Step 2: Replace the installed app**

  ```bash
  rm -rf "/Applications/Residuum Chat.app"
  cp -R "/tmp/residuum-chat-build/Build/Products/Release/Residuum Chat.app" /Applications/
  ```

- [ ] **Step 3: Smoke test**

  Launch the app, open the popover, type `/` in the input bar. Verify:
  - Menu appears above the input with all 8 commands
  - Typing `/ob` narrows to `/observe`
  - ↑/↓ arrows move the highlight
  - Enter selects and executes
  - Escape dismisses
  - `/help` appends a monospace block to the chat feed
  - `/inbox ` populates the field for argument entry

- [ ] **Step 4: Final commit**

  ```bash
  cd /Users/jflinn/projects/residuum
  git add mac/
  git commit -m "feat(mac): slash commands complete — build and install"
  ```

---

## Done

Slash commands are live. Type `/` in any agent tab to see the command menu. All 8 commands work: 3 client-side (`/help`, `/verbose`, `/status`) append styled messages to the feed; 5 send WebSocket frames to the daemon (`/observe`, `/reflect`, `/context`, `/reload`, `/inbox`).
