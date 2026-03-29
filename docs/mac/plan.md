# Residuum Mac App Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a native SwiftUI macOS menu bar app that connects to the Residuum daemon via WebSocket and provides a chat interface for interacting with personal AI agents.

**Architecture:** `NSStatusItem` + `NSPopover` for the menu bar icon and quick-access chat. One `ResiduumConnection` (URLSessionWebSocketTask) per agent tab, coordinated by an `@Observable` `AgentStore`. Agent list is discovered from `~/.residuum/agent_registry/registry.toml`.

**Tech Stack:** Swift 5.9+, SwiftUI, AppKit (NSStatusItem/NSPopover/NSWindow), URLSession WebSocket, XCTest, xcodegen

**Spec:** `docs/mac/design.md`

---

## File Map

```
mac/
├── project.yml                        # xcodegen project spec
├── Residuum/
│   ├── App/
│   │   ├── ResiduumApp.swift          # @main, injects AgentStore into environment
│   │   └── AppDelegate.swift          # NSStatusItem, NSPopover, NSWindow lifecycle
│   ├── Connection/
│   │   ├── Protocol.swift             # ClientMessage + ServerMessage Codable enums, ImageData
│   │   └── ResiduumConnection.swift   # WebSocket lifecycle, ping, reconnect backoff
│   ├── Agents/
│   │   ├── Message.swift              # ChatMessage, ToolCallData value types
│   │   ├── AgentRegistry.swift        # Reads ~/.residuum/agent_registry/registry.toml
│   │   ├── AgentTab.swift             # AgentTab struct (name, port, connection, messages)
│   │   └── AgentStore.swift           # @Observable, owns all tabs + connections
│   ├── Views/
│   │   ├── Style.swift                # Colors, fonts, vein divider — single source of truth
│   │   ├── PopoverView.swift          # Root view (header + chat + input + expand button)
│   │   ├── TabBar.swift               # Pill-style agent tab switcher
│   │   ├── ChatView.swift             # Scrollable message list
│   │   ├── MessageRow.swift           # User / assistant / system message rendering
│   │   ├── ThinkingIndicator.swift    # Animated three-dot indicator
│   │   ├── ToolGroup.swift            # Collapsible tool call visualization
│   │   ├── InputBar.swift             # Text input, file chips, send button, NSOpenPanel
│   │   └── SettingsView.swift         # Host/port fields, connection status, UserDefaults
│   ├── Resources/
│   │   ├── Assets.xcassets/           # StatusIcon template image
│   │   └── Fonts/                     # Cinzel, Literata, JetBrains Mono TTF files
│   └── Info.plist                     # LSUIElement=YES, NSAppTransportSecurity
└── ResiduumTests/
    ├── ProtocolTests.swift            # Encode/decode round-trip for all message types
    └── AgentRegistryTests.swift       # TOML parsing, edge cases
```

---

## Task 1: Prerequisites — Install Xcode and xcodegen

**Files:** none

- [ ] **Step 1: Install Xcode**

  Open the App Store, search "Xcode", and install it. This is ~10 GB and will take a while.

  After installing, open Xcode once to accept the license agreement and let it install components.

- [ ] **Step 2: Verify Xcode command-line tools**

  ```bash
  xcodebuild -version
  ```

  Expected: `Xcode 16.x` (or later)

- [ ] **Step 3: Install xcodegen via Homebrew**

  ```bash
  brew install xcodegen
  xcodegen --version
  ```

  Expected: `XcodeGen Version: 2.x.x`

---

## Task 2: Scaffold the Xcode project

**Files:**
- Create: `mac/project.yml`
- Create: `mac/Residuum/Info.plist`
- Create: `mac/Residuum/App/ResiduumApp.swift` (stub)
- Create: `mac/ResiduumTests/ProtocolTests.swift` (stub)

- [ ] **Step 1: Create the mac directory**

  ```bash
  mkdir -p residuum/mac/Residuum/App
  mkdir -p residuum/mac/Residuum/Connection
  mkdir -p residuum/mac/Residuum/Agents
  mkdir -p residuum/mac/Residuum/Views
  mkdir -p residuum/mac/Residuum/Resources/Assets.xcassets/StatusIcon.imageset
  mkdir -p residuum/mac/Residuum/Resources/Fonts
  mkdir -p residuum/mac/ResiduumTests
  ```

- [ ] **Step 2: Write project.yml**

  Create `mac/project.yml`:

  ```yaml
  name: Residuum
  options:
    bundleIdPrefix: com.grizzly-endeavors
    deploymentTarget:
      macOS: "14.0"
    xcodeVersion: "16.0"
    createIntermediateGroups: true

  targets:
    Residuum:
      type: application
      platform: macOS
      sources:
        - path: Residuum
      settings:
        base:
          PRODUCT_BUNDLE_IDENTIFIER: com.grizzly-endeavors.residuum-mac
          MARKETING_VERSION: 1.0.0
          CURRENT_PROJECT_VERSION: 1
          ENABLE_APP_SANDBOX: NO
          INFOPLIST_FILE: Residuum/Info.plist
          SWIFT_VERSION: "5.9"
      preBuildScripts: []

    ResiduumTests:
      type: bundle.unit-test
      platform: macOS
      sources:
        - path: ResiduumTests
      dependencies:
        - target: Residuum
      settings:
        base:
          SWIFT_VERSION: "5.9"
  ```

- [ ] **Step 3: Write Info.plist**

  Create `mac/Residuum/Info.plist`:

  ```xml
  <?xml version="1.0" encoding="UTF-8"?>
  <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
  <plist version="1.0">
  <dict>
      <key>CFBundleName</key>
      <string>Residuum</string>
      <key>CFBundleDisplayName</key>
      <string>Residuum</string>
      <key>CFBundleIdentifier</key>
      <string>com.grizzly-endeavors.residuum-mac</string>
      <key>CFBundleVersion</key>
      <string>1</string>
      <key>CFBundleShortVersionString</key>
      <string>1.0.0</string>
      <key>LSMinimumSystemVersion</key>
      <string>14.0</string>
      <key>LSUIElement</key>
      <true/>
      <key>NSAppTransportSecurity</key>
      <dict>
          <key>NSAllowsLocalNetworking</key>
          <true/>
      </dict>
      <key>CFBundleFonts</key>
      <array>
          <string>Cinzel-Regular.ttf</string>
          <string>Cinzel-SemiBold.ttf</string>
          <string>Literata-Light.ttf</string>
          <string>Literata-LightItalic.ttf</string>
          <string>JetBrainsMono-Regular.ttf</string>
      </array>
  </dict>
  </plist>
  ```

  `LSUIElement = true` hides the app from the Dock and from ⌘-Tab. `NSAllowsLocalNetworking` permits WebSocket connections to localhost without full ATS bypass.

- [ ] **Step 4: Write the minimal app stub**

  Create `mac/Residuum/App/ResiduumApp.swift`:

  ```swift
  import SwiftUI

  @main
  struct ResiduumApp: App {
      @NSApplicationDelegateAdaptor(AppDelegate.self) var appDelegate

      var body: some Scene {
          // No windows — this app lives entirely in the menu bar.
          // The Settings scene is required to suppress the "no scenes" warning.
          Settings { EmptyView() }
      }
  }
  ```

  Create `mac/Residuum/App/AppDelegate.swift`:

  ```swift
  import AppKit
  import SwiftUI

  class AppDelegate: NSObject, NSApplicationDelegate {
      var statusItem: NSStatusItem!
      var popover: NSPopover!

      func applicationDidFinishLaunching(_ notification: Notification) {
          statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
          if let button = statusItem.button {
              button.image = NSImage(systemSymbolName: "circle.hexagonpath.fill",
                                    accessibilityDescription: "Residuum")
              button.image?.isTemplate = true
              button.action = #selector(togglePopover)
              button.target = self
          }

          popover = NSPopover()
          popover.contentSize = NSSize(width: 420, height: 520)
          popover.behavior = .transient
          popover.contentViewController = NSHostingController(rootView: Text("Hello"))
      }

      @objc func togglePopover() {
          guard let button = statusItem.button else { return }
          if popover.isShown {
              popover.performClose(nil)
          } else {
              popover.show(relativeTo: button.bounds, of: button, preferredEdge: .minY)
          }
      }
  }
  ```

- [ ] **Step 5: Write a placeholder test file**

  Create `mac/ResiduumTests/ProtocolTests.swift`:

  ```swift
  import XCTest

  final class ProtocolTests: XCTestCase {
      func testPlaceholder() {
          // Will be replaced in Task 3
          XCTAssertTrue(true)
      }
  }
  ```

- [ ] **Step 6: Generate the Xcode project**

  ```bash
  cd residuum/mac
  xcodegen generate
  ```

  Expected output ends with: `✓ Generated: Residuum.xcodeproj`

- [ ] **Step 7: Build and confirm it compiles**

  ```bash
  cd residuum/mac
  xcodebuild -scheme Residuum -configuration Debug build 2>&1 | tail -5
  ```

  Expected: `** BUILD SUCCEEDED **`

- [ ] **Step 8: Commit**

  ```bash
  cd residuum
  git add mac/
  git commit -m "feat(mac): scaffold Xcode project"
  ```

---

## Task 3: Protocol types

**Files:**
- Create: `mac/Residuum/Connection/Protocol.swift`
- Modify: `mac/ResiduumTests/ProtocolTests.swift`

The Swift enums mirror the Rust types exactly, using the same `snake_case` JSON tag names.

- [ ] **Step 1: Write the failing tests**

  Replace `mac/ResiduumTests/ProtocolTests.swift`:

  ```swift
  import XCTest
  @testable import Residuum

  final class ProtocolTests: XCTestCase {
      private let encoder = JSONEncoder()
      private let decoder = JSONDecoder()

      // MARK: - ClientMessage encoding

      func testEncodeSendMessage() throws {
          let msg = ClientMessage.sendMessage(id: "abc", content: "hello", images: [])
          let data = try encoder.encode(msg)
          let json = try JSONSerialization.jsonObject(with: data) as! [String: Any]
          XCTAssertEqual(json["type"] as? String, "send_message")
          XCTAssertEqual(json["id"] as? String, "abc")
          XCTAssertEqual(json["content"] as? String, "hello")
          XCTAssertNil(json["images"], "empty images array should be omitted")
      }

      func testEncodeSendMessageWithImage() throws {
          let image = ImageData(mediaType: "image/png", data: "base64data")
          let msg = ClientMessage.sendMessage(id: "x", content: "look", images: [image])
          let data = try encoder.encode(msg)
          let json = try JSONSerialization.jsonObject(with: data) as! [String: Any]
          let images = json["images"] as? [[String: Any]]
          XCTAssertEqual(images?.count, 1)
          XCTAssertEqual(images?.first?["media_type"] as? String, "image/png")
          XCTAssertEqual(images?.first?["data"] as? String, "base64data")
      }

      func testEncodeSetVerbose() throws {
          let msg = ClientMessage.setVerbose(enabled: true)
          let data = try encoder.encode(msg)
          let json = try JSONSerialization.jsonObject(with: data) as! [String: Any]
          XCTAssertEqual(json["type"] as? String, "set_verbose")
          XCTAssertEqual(json["enabled"] as? Bool, true)
      }

      func testEncodePing() throws {
          let msg = ClientMessage.ping
          let data = try encoder.encode(msg)
          let json = try JSONSerialization.jsonObject(with: data) as! [String: Any]
          XCTAssertEqual(json["type"] as? String, "ping")
      }

      func testEncodeServerCommand() throws {
          let msg = ClientMessage.serverCommand(name: "observe", args: nil)
          let data = try encoder.encode(msg)
          let json = try JSONSerialization.jsonObject(with: data) as! [String: Any]
          XCTAssertEqual(json["type"] as? String, "server_command")
          XCTAssertEqual(json["name"] as? String, "observe")
          XCTAssertNil(json["args"])
      }

      // MARK: - ServerMessage decoding

      func testDecodeTurnStarted() throws {
          let json = #"{"type":"turn_started","reply_to":"corr-1"}"#.data(using: .utf8)!
          let msg = try decoder.decode(ServerMessage.self, from: json)
          guard case .turnStarted(let replyTo) = msg else {
              return XCTFail("expected turnStarted, got \(msg)")
          }
          XCTAssertEqual(replyTo, "corr-1")
      }

      func testDecodeResponse() throws {
          let json = #"{"type":"response","reply_to":"corr-1","content":"Hello there"}"#.data(using: .utf8)!
          let msg = try decoder.decode(ServerMessage.self, from: json)
          guard case .response(let replyTo, let content) = msg else {
              return XCTFail("expected response, got \(msg)")
          }
          XCTAssertEqual(replyTo, "corr-1")
          XCTAssertEqual(content, "Hello there")
      }

      func testDecodeToolCall() throws {
          let json = #"{"type":"tool_call","id":"tc1","name":"search","arguments":{"q":"test"}}"#.data(using: .utf8)!
          let msg = try decoder.decode(ServerMessage.self, from: json)
          guard case .toolCall(let id, let name, _) = msg else {
              return XCTFail("expected toolCall, got \(msg)")
          }
          XCTAssertEqual(id, "tc1")
          XCTAssertEqual(name, "search")
      }

      func testDecodeToolResult() throws {
          let json = #"{"type":"tool_result","tool_call_id":"tc1","name":"search","output":"found it","is_error":false}"#.data(using: .utf8)!
          let msg = try decoder.decode(ServerMessage.self, from: json)
          guard case .toolResult(let tcId, let name, let output, let isError) = msg else {
              return XCTFail("expected toolResult, got \(msg)")
          }
          XCTAssertEqual(tcId, "tc1")
          XCTAssertEqual(name, "search")
          XCTAssertEqual(output, "found it")
          XCTAssertFalse(isError)
      }

      func testDecodeError() throws {
          let json = #"{"type":"error","reply_to":"corr-1","message":"something went wrong"}"#.data(using: .utf8)!
          let msg = try decoder.decode(ServerMessage.self, from: json)
          guard case .error(let replyTo, let message) = msg else {
              return XCTFail("expected error, got \(msg)")
          }
          XCTAssertEqual(replyTo, "corr-1")
          XCTAssertEqual(message, "something went wrong")
      }

      func testDecodeErrorWithNilReplyTo() throws {
          let json = #"{"type":"error","message":"something went wrong"}"#.data(using: .utf8)!
          let msg = try decoder.decode(ServerMessage.self, from: json)
          guard case .error(let replyTo, _) = msg else {
              return XCTFail("expected error, got \(msg)")
          }
          XCTAssertNil(replyTo)
      }

      func testDecodeUnknownTypeDoesNotThrow() throws {
          let json = #"{"type":"future_message","data":"whatever"}"#.data(using: .utf8)!
          let msg = try decoder.decode(ServerMessage.self, from: json)
          guard case .unknown = msg else {
              return XCTFail("expected unknown, got \(msg)")
          }
      }
  }
  ```

- [ ] **Step 2: Run tests — expect failure (type not defined yet)**

  ```bash
  cd residuum/mac
  xcodebuild test -scheme Residuum -destination 'platform=macOS' 2>&1 | grep -E "error:|FAILED|PASSED" | head -10
  ```

  Expected: build errors about `ClientMessage`, `ServerMessage`, `ImageData` not being defined.

- [ ] **Step 3: Write Protocol.swift**

  Create `mac/Residuum/Connection/Protocol.swift`:

  ```swift
  import Foundation

  // MARK: - ImageData

  /// Base64-encoded image attachment. Matches the Rust `ImageData` struct exactly.
  struct ImageData: Codable {
      /// MIME type, e.g. `"image/png"`, `"image/jpeg"`.
      let mediaType: String
      /// Base64-encoded image bytes.
      let data: String

      enum CodingKeys: String, CodingKey {
          case mediaType = "media_type"
          case data
      }
  }

  // MARK: - ClientMessage

  /// Messages sent from this app to the Residuum daemon.
  /// Matches the Rust `ClientMessage` enum with `snake_case` JSON tags.
  enum ClientMessage: Encodable {
      case sendMessage(id: String, content: String, images: [ImageData])
      case setVerbose(enabled: Bool)
      case ping
      case reload
      case serverCommand(name: String, args: String?)
      case inboxAdd(body: String)

      private enum CodingKeys: String, CodingKey {
          case type, id, content, images, enabled, name, args, body
      }

      func encode(to encoder: Encoder) throws {
          var c = encoder.container(keyedBy: CodingKeys.self)
          switch self {
          case .sendMessage(let id, let content, let images):
              try c.encode("send_message", forKey: .type)
              try c.encode(id, forKey: .id)
              try c.encode(content, forKey: .content)
              if !images.isEmpty {
                  try c.encode(images, forKey: .images)
              }
          case .setVerbose(let enabled):
              try c.encode("set_verbose", forKey: .type)
              try c.encode(enabled, forKey: .enabled)
          case .ping:
              try c.encode("ping", forKey: .type)
          case .reload:
              try c.encode("reload", forKey: .type)
          case .serverCommand(let name, let args):
              try c.encode("server_command", forKey: .type)
              try c.encode(name, forKey: .name)
              try c.encodeIfPresent(args, forKey: .args)
          case .inboxAdd(let body):
              try c.encode("inbox_add", forKey: .type)
              try c.encode(body, forKey: .body)
          }
      }
  }

  // MARK: - ServerMessage

  /// Messages received from the Residuum daemon.
  /// Matches the Rust `ServerMessage` enum with `snake_case` JSON tags.
  enum ServerMessage: Decodable {
      case turnStarted(replyTo: String)
      case toolCall(id: String, name: String, arguments: [String: JSONValue])
      case toolResult(toolCallId: String, name: String, output: String, isError: Bool)
      case response(replyTo: String, content: String)
      case systemEvent(source: String, content: String)
      case broadcastResponse(content: String)
      case error(replyTo: String?, message: String)
      case notice(message: String)
      case pong
      case unknown

      private enum CodingKeys: String, CodingKey {
          case type
          case replyTo = "reply_to"
          case id, name, arguments
          case toolCallId = "tool_call_id"
          case output
          case isError = "is_error"
          case source, content, message
      }

      init(from decoder: Decoder) throws {
          let c = try decoder.container(keyedBy: CodingKeys.self)
          let type = try c.decode(String.self, forKey: .type)
          switch type {
          case "turn_started":
              self = .turnStarted(replyTo: try c.decode(String.self, forKey: .replyTo))
          case "tool_call":
              self = .toolCall(
                  id: try c.decode(String.self, forKey: .id),
                  name: try c.decode(String.self, forKey: .name),
                  arguments: (try? c.decode([String: JSONValue].self, forKey: .arguments)) ?? [:]
              )
          case "tool_result":
              self = .toolResult(
                  toolCallId: try c.decode(String.self, forKey: .toolCallId),
                  name: try c.decode(String.self, forKey: .name),
                  output: try c.decode(String.self, forKey: .output),
                  isError: try c.decode(Bool.self, forKey: .isError)
              )
          case "response":
              self = .response(
                  replyTo: try c.decode(String.self, forKey: .replyTo),
                  content: try c.decode(String.self, forKey: .content)
              )
          case "system_event":
              self = .systemEvent(
                  source: try c.decode(String.self, forKey: .source),
                  content: try c.decode(String.self, forKey: .content)
              )
          case "broadcast_response":
              self = .broadcastResponse(content: try c.decode(String.self, forKey: .content))
          case "error":
              self = .error(
                  replyTo: try? c.decode(String.self, forKey: .replyTo),
                  message: try c.decode(String.self, forKey: .message)
              )
          case "notice":
              self = .notice(message: try c.decode(String.self, forKey: .message))
          case "pong":
              self = .pong
          default:
              self = .unknown
          }
      }
  }

  // MARK: - JSONValue

  /// A type-erased JSON value for decoding tool call arguments,
  /// which can be any valid JSON structure.
  enum JSONValue: Decodable, CustomStringConvertible {
      case string(String)
      case number(Double)
      case bool(Bool)
      case null
      case array([JSONValue])
      case object([String: JSONValue])

      init(from decoder: Decoder) throws {
          let c = try decoder.singleValueContainer()
          if let v = try? c.decode(String.self)  { self = .string(v); return }
          if let v = try? c.decode(Double.self)  { self = .number(v); return }
          if let v = try? c.decode(Bool.self)    { self = .bool(v); return }
          if let v = try? c.decode([JSONValue].self) { self = .array(v); return }
          if let v = try? c.decode([String: JSONValue].self) { self = .object(v); return }
          self = .null
      }

      var description: String {
          switch self {
          case .string(let s): return s
          case .number(let n): return n.truncatingRemainder(dividingBy: 1) == 0
              ? String(Int(n)) : String(n)
          case .bool(let b): return b ? "true" : "false"
          case .null: return "null"
          case .array(let a): return "[\(a.map(\.description).joined(separator: ", "))]"
          case .object(let o): return "{\(o.map { "\($0.key): \($0.value)" }.joined(separator: ", "))}"
          }
      }
  }
  ```

- [ ] **Step 4: Run tests — expect passing**

  ```bash
  cd residuum/mac
  xcodebuild test -scheme Residuum -destination 'platform=macOS' 2>&1 | grep -E "Test Suite|passed|failed" | tail -5
  ```

  Expected: `Test Suite 'ProtocolTests' passed`

- [ ] **Step 5: Commit**

  ```bash
  cd residuum
  git add mac/Residuum/Connection/Protocol.swift mac/ResiduumTests/ProtocolTests.swift
  git commit -m "feat(mac): add WebSocket protocol types with codec tests"
  ```

---

## Task 4: AgentRegistry reader

**Files:**
- Create: `mac/Residuum/Agents/AgentRegistry.swift`
- Create: `mac/ResiduumTests/AgentRegistryTests.swift`

- [ ] **Step 1: Write the failing tests**

  Create `mac/ResiduumTests/AgentRegistryTests.swift`:

  ```swift
  import XCTest
  @testable import Residuum

  final class AgentRegistryTests: XCTestCase {

      func testParsesEmptyFile() throws {
          let registry = try AgentRegistry.parse("")
          XCTAssertTrue(registry.agents.isEmpty)
      }

      func testParsesSingleAgent() throws {
          let toml = """
          [[agents]]
          name = "Aria"
          port = 7701
          """
          let registry = try AgentRegistry.parse(toml)
          XCTAssertEqual(registry.agents.count, 1)
          XCTAssertEqual(registry.agents[0].name, "Aria")
          XCTAssertEqual(registry.agents[0].port, 7701)
      }

      func testParsesMultipleAgents() throws {
          let toml = """
          [[agents]]
          name = "Aria"
          port = 7701

          [[agents]]
          name = "Sentinel"
          port = 7702
          """
          let registry = try AgentRegistry.parse(toml)
          XCTAssertEqual(registry.agents.count, 2)
          XCTAssertEqual(registry.agents[1].name, "Sentinel")
          XCTAssertEqual(registry.agents[1].port, 7702)
      }

      func testIgnoresUnknownKeys() throws {
          let toml = """
          [[agents]]
          name = "Aria"
          port = 7701
          description = "my agent"
          """
          let registry = try AgentRegistry.parse(toml)
          XCTAssertEqual(registry.agents.count, 1)
      }

      func testMissingPortIsSkipped() throws {
          let toml = """
          [[agents]]
          name = "Broken"
          """
          let registry = try AgentRegistry.parse(toml)
          XCTAssertTrue(registry.agents.isEmpty, "agents without a port should be skipped")
      }

      func testRegistryURLContainsExpectedPath() {
          let url = AgentRegistry.registryURL()
          XCTAssertNotNil(url)
          XCTAssertTrue(url!.path.contains(".residuum/agent_registry/registry.toml"))
      }
  }
  ```

- [ ] **Step 2: Run tests — expect failure**

  ```bash
  cd residuum/mac
  xcodebuild test -scheme Residuum -destination 'platform=macOS' 2>&1 | grep -E "error:|FAILED" | head -5
  ```

  Expected: build errors about `AgentRegistry` not defined.

- [ ] **Step 3: Write AgentRegistry.swift**

  Create `mac/Residuum/Agents/AgentRegistry.swift`:

  ```swift
  import Foundation

  /// A single named agent entry from the registry.
  struct AgentEntry: Equatable {
      let name: String
      let port: UInt16
  }

  /// Reads the Residuum agent registry from
  /// `~/.residuum/agent_registry/registry.toml`.
  ///
  /// The file format is a TOML array of tables:
  /// ```toml
  /// [[agents]]
  /// name = "Aria"
  /// port = 7701
  /// ```
  struct AgentRegistry {
      let agents: [AgentEntry]

      /// Load the registry from the standard location.
      /// Returns an empty registry if the file doesn't exist.
      static func load() -> AgentRegistry {
          guard let url = registryURL(),
                let contents = try? String(contentsOf: url, encoding: .utf8) else {
              return AgentRegistry(agents: [])
          }
          return (try? parse(contents)) ?? AgentRegistry(agents: [])
      }

      /// Parse a TOML string into an `AgentRegistry`.
      /// Only handles the specific format used by Residuum's registry.
      static func parse(_ toml: String) throws -> AgentRegistry {
          var agents: [AgentEntry] = []
          var currentName: String?
          var currentPort: UInt16?

          func flushCurrent() {
              if let name = currentName, let port = currentPort {
                  agents.append(AgentEntry(name: name, port: port))
              }
              currentName = nil
              currentPort = nil
          }

          for line in toml.components(separatedBy: .newlines) {
              let trimmed = line.trimmingCharacters(in: .whitespaces)
              if trimmed == "[[agents]]" {
                  flushCurrent()
              } else if trimmed.hasPrefix("name = ") {
                  currentName = trimmed
                      .dropFirst("name = ".count)
                      .trimmingCharacters(in: CharacterSet(charactersIn: "\""))
              } else if trimmed.hasPrefix("port = ") {
                  let raw = String(trimmed.dropFirst("port = ".count))
                      .trimmingCharacters(in: .whitespaces)
                  currentPort = UInt16(raw)
              }
          }
          flushCurrent()

          return AgentRegistry(agents: agents)
      }

      /// URL of the registry file: `~/.residuum/agent_registry/registry.toml`.
      static func registryURL() -> URL? {
          guard let home = ProcessInfo.processInfo.environment["HOME"] else { return nil }
          return URL(fileURLWithPath: home)
              .appendingPathComponent(".residuum/agent_registry/registry.toml")
      }
  }
  ```

- [ ] **Step 4: Run tests — expect passing**

  ```bash
  cd residuum/mac
  xcodebuild test -scheme Residuum -destination 'platform=macOS' 2>&1 | grep -E "Test Suite|passed|failed" | tail -5
  ```

  Expected: all tests pass.

- [ ] **Step 5: Commit**

  ```bash
  cd residuum
  git add mac/Residuum/Agents/AgentRegistry.swift mac/ResiduumTests/AgentRegistryTests.swift
  git commit -m "feat(mac): add AgentRegistry TOML reader with tests"
  ```

---

## Task 5: Message model and AgentTab

**Files:**
- Create: `mac/Residuum/Agents/Message.swift`
- Create: `mac/Residuum/Agents/AgentTab.swift`

No tests needed — these are pure value types with no behaviour.

- [ ] **Step 1: Write Message.swift**

  Create `mac/Residuum/Agents/Message.swift`:

  ```swift
  import Foundation

  /// A single message in an agent's conversation.
  struct ChatMessage: Identifiable {
      let id: UUID
      let role: Role
      /// The text content of the message.
      var content: String
      /// Tool calls associated with this agent turn (assistant messages only).
      var toolCalls: [ToolCallData]

      enum Role {
          case user
          case assistant
          case system   // notices and system events
      }

      init(id: UUID = UUID(), role: Role, content: String, toolCalls: [ToolCallData] = []) {
          self.id = id
          self.role = role
          self.content = content
          self.toolCalls = toolCalls
      }
  }

  /// A tool invocation and its result within an assistant turn.
  struct ToolCallData: Identifiable {
      /// The tool_call_id from the daemon.
      let id: String
      let name: String
      let arguments: [String: JSONValue]
      /// Populated when the tool_result arrives.
      var result: String?
      var isError: Bool
  }
  ```

- [ ] **Step 2: Write AgentTab.swift**

  Create `mac/Residuum/Agents/AgentTab.swift`:

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

- [ ] **Step 3: Build to confirm no errors**

  ```bash
  cd residuum/mac
  xcodebuild -scheme Residuum -configuration Debug build 2>&1 | tail -3
  ```

  Expected: `** BUILD SUCCEEDED **`

- [ ] **Step 4: Commit**

  ```bash
  cd residuum
  git add mac/Residuum/Agents/Message.swift mac/Residuum/Agents/AgentTab.swift
  git commit -m "feat(mac): add ChatMessage and AgentTab value types"
  ```

---

## Task 6: ResiduumConnection — WebSocket lifecycle

**Files:**
- Create: `mac/Residuum/Connection/ResiduumConnection.swift`

`ResiduumConnection` is a class (reference type) because it holds async state and conforms to `URLSessionWebSocketDelegate`. It is NOT `@Observable` — `AgentStore` observes it via a callback.

- [ ] **Step 1: Write ResiduumConnection.swift**

  Create `mac/Residuum/Connection/ResiduumConnection.swift`:

  ```swift
  import Foundation

  enum ConnectionState: Equatable {
      case connecting
      case connected
      case disconnected
  }

  /// Manages a single WebSocket connection to one Residuum agent daemon.
  ///
  /// Call `connect()` to start. The connection retries automatically on
  /// failure with exponential backoff (1 s → 2 s → 4 s → … → 30 s max).
  /// Call `disconnect()` to stop permanently.
  ///
  /// Received messages are delivered on the main queue via `onMessage`.
  final class ResiduumConnection: NSObject {
      // MARK: - Public state

      private(set) var state: ConnectionState = .disconnected

      /// Called on the main queue whenever a message arrives from the daemon.
      var onMessage: ((ServerMessage) -> Void)?

      /// Called on the main queue whenever the connection state changes.
      var onStateChange: ((ConnectionState) -> Void)?

      // MARK: - Private

      private let host: String
      private let port: UInt16
      private var task: URLSessionWebSocketTask?
      private lazy var session: URLSession = URLSession(
          configuration: .default,
          delegate: self,
          delegateQueue: nil
      )
      private var pingTimer: Timer?
      private var retryDelay: TimeInterval = 1.0
      private var active = false  // false → don't reconnect after close

      // MARK: - Init

      init(host: String, port: UInt16) {
          self.host = host
          self.port = port
      }

      // MARK: - Public API

      func connect() {
          active = true
          openConnection()
      }

      func disconnect() {
          active = false
          pingTimer?.invalidate()
          pingTimer = nil
          task?.cancel(with: .normalClosure, reason: nil)
          task = nil
          updateState(.disconnected)
      }

      func send(_ message: ClientMessage) {
          guard state == .connected, let task else { return }
          guard let data = try? JSONEncoder().encode(message) else { return }
          task.send(.data(data)) { _ in }
      }

      // MARK: - Private

      private func openConnection() {
          var components = URLComponents()
          components.scheme = "ws"
          components.host = host
          components.port = Int(port)
          components.path = "/ws"
          guard let url = components.url else { return }

          updateState(.connecting)
          let t = session.webSocketTask(with: url)
          self.task = t
          t.resume()
          receiveNext()
      }

      private func receiveNext() {
          task?.receive { [weak self] result in
              guard let self else { return }
              switch result {
              case .success(let wsMessage):
                  let data: Data?
                  switch wsMessage {
                  case .data(let d):   data = d
                  case .string(let s): data = s.data(using: .utf8)
                  @unknown default:    data = nil
                  }
                  if let data, let msg = try? JSONDecoder().decode(ServerMessage.self, from: data) {
                      DispatchQueue.main.async { self.onMessage?(msg) }
                  }
                  self.receiveNext()
              case .failure:
                  self.scheduleReconnect()
              }
          }
      }

      private func scheduleReconnect() {
          DispatchQueue.main.async {
              self.updateState(.disconnected)
              self.pingTimer?.invalidate()
              self.pingTimer = nil
              guard self.active else { return }
              DispatchQueue.main.asyncAfter(deadline: .now() + self.retryDelay) {
                  guard self.active else { return }
                  self.retryDelay = min(self.retryDelay * 2, 30)
                  self.openConnection()
              }
          }
      }

      private func updateState(_ newState: ConnectionState) {
          guard state != newState else { return }
          state = newState
          onStateChange?(newState)
      }

      private func startPing() {
          pingTimer = Timer.scheduledTimer(withTimeInterval: 30, repeats: true) { [weak self] _ in
              self?.send(.ping)
          }
      }
  }

  // MARK: - URLSessionWebSocketDelegate

  extension ResiduumConnection: URLSessionWebSocketDelegate {
      func urlSession(
          _ session: URLSession,
          webSocketTask: URLSessionWebSocketTask,
          didOpenWithProtocol protocol: String?
      ) {
          DispatchQueue.main.async {
              self.retryDelay = 1.0
              self.updateState(.connected)
              self.send(.setVerbose(enabled: true))
              self.startPing()
          }
      }

      func urlSession(
          _ session: URLSession,
          webSocketTask: URLSessionWebSocketTask,
          didCloseWith closeCode: URLSessionWebSocketTask.CloseCode,
          reason: Data?
      ) {
          scheduleReconnect()
      }
  }
  ```

- [ ] **Step 2: Build**

  ```bash
  cd residuum/mac
  xcodebuild -scheme Residuum -configuration Debug build 2>&1 | tail -3
  ```

  Expected: `** BUILD SUCCEEDED **`

- [ ] **Step 3: Commit**

  ```bash
  cd residuum
  git add mac/Residuum/Connection/ResiduumConnection.swift
  git commit -m "feat(mac): add ResiduumConnection WebSocket client"
  ```

---

## Task 7: AgentStore

**Files:**
- Create: `mac/Residuum/Agents/AgentStore.swift`

`AgentStore` is `@Observable` — SwiftUI views watch it directly. It owns all `AgentTab`s (and their connections).

- [ ] **Step 1: Write AgentStore.swift**

  Create `mac/Residuum/Agents/AgentStore.swift`:

  ```swift
  import Foundation
  import Observation

  /// Central store for all agent tabs and their conversations.
  ///
  /// Inject into the SwiftUI environment via `.environment(agentStore)` and
  /// read with `@Environment(AgentStore.self)` in views.
  @Observable
  final class AgentStore {
      // MARK: - Public state (observed by SwiftUI)

      var tabs: [AgentTab] = []
      var selectedTabId: UUID?
      var host: String

      var selectedTab: AgentTab? {
          guard let id = selectedTabId else { return tabs.first }
          return tabs.first { $0.id == id }
      }

      var selectedTabIndex: Int? {
          guard let id = selectedTabId else { return tabs.isEmpty ? nil : 0 }
          return tabs.firstIndex { $0.id == id }
      }

      // MARK: - Init

      init(host: String = "127.0.0.1") {
          self.host = host
          loadAgents()
      }

      // MARK: - Public API

      func select(_ tab: AgentTab) {
          selectedTabId = tab.id
      }

      /// Send a message on the currently selected agent tab.
      func sendMessage(content: String, images: [ImageData] = []) {
          guard let idx = selectedTabIndex else { return }
          let correlationId = UUID().uuidString
          let userMsg = ChatMessage(role: .user, content: content)
          tabs[idx].messages.append(userMsg)
          tabs[idx].pendingCorrelationId = correlationId
          tabs[idx].connection.send(.sendMessage(id: correlationId, content: content, images: images))
      }

      /// Force-reconnect a specific tab's connection.
      func reconnect(tab: AgentTab) {
          guard let idx = tabs.firstIndex(where: { $0.id == tab.id }) else { return }
          tabs[idx].connection.disconnect()
          tabs[idx].connection.connect()
      }

      // MARK: - Setup

      private func loadAgents() {
          // Default agent is always first (port 7700).
          let defaultConn = makeConnection(port: 7700)
          let defaultTab = AgentTab(name: "Default", port: 7700, connection: defaultConn)
          tabs.append(defaultTab)

          // Named agents from registry.
          let registry = AgentRegistry.load()
          for entry in registry.agents {
              let conn = makeConnection(port: entry.port)
              let tab = AgentTab(name: entry.name, port: entry.port, connection: conn)
              tabs.append(tab)
          }

          selectedTabId = tabs.first?.id

          // Wire up handlers and connect all.
          for idx in tabs.indices {
              wireHandlers(tabIndex: idx)
              tabs[idx].connection.connect()
          }
      }

      private func makeConnection(port: UInt16) -> ResiduumConnection {
          ResiduumConnection(host: host, port: port)
      }

      private func wireHandlers(tabIndex: Int) {
          tabs[tabIndex].connection.onMessage = { [weak self] message in
              self?.handle(message, tabIndex: tabIndex)
          }
          tabs[tabIndex].connection.onStateChange = { [weak self] _ in
              // Triggers @Observable change so views re-render connection status.
              _ = self?.tabs[tabIndex].connection.state
          }
      }

      // MARK: - Message handling

      private func handle(_ message: ServerMessage, tabIndex: Int) {
          switch message {
          case .turnStarted(let correlationId):
              tabs[tabIndex].isThinking = true
              tabs[tabIndex].pendingCorrelationId = correlationId
              tabs[tabIndex].pendingToolCalls = []

          case .toolCall(let id, let name, let arguments):
              let call = ToolCallData(id: id, name: name, arguments: arguments, isError: false)
              tabs[tabIndex].pendingToolCalls.append(call)

          case .toolResult(let toolCallId, _, let output, let isError):
              if let idx = tabs[tabIndex].pendingToolCalls.firstIndex(where: { $0.id == toolCallId }) {
                  tabs[tabIndex].pendingToolCalls[idx].result = output
                  tabs[tabIndex].pendingToolCalls[idx].isError = isError
              }

          case .response(_, let content):
              var assistantMsg = ChatMessage(role: .assistant, content: content)
              assistantMsg.toolCalls = tabs[tabIndex].pendingToolCalls
              tabs[tabIndex].messages.append(assistantMsg)
              tabs[tabIndex].isThinking = false
              tabs[tabIndex].pendingToolCalls = []
              tabs[tabIndex].pendingCorrelationId = nil

          case .broadcastResponse(let content):
              // Intermediate text — append as a partial assistant message.
              // Only add if not already thinking (avoids duplicates with final response).
              if tabs[tabIndex].isThinking {
                  // Don't append — let the final response replace it.
                  break
              }
              let msg = ChatMessage(role: .assistant, content: content)
              tabs[tabIndex].messages.append(msg)

          case .systemEvent(let source, let content):
              let msg = ChatMessage(role: .system, content: "[\(source)] \(content)")
              tabs[tabIndex].messages.append(msg)

          case .notice(let message):
              let msg = ChatMessage(role: .system, content: message)
              tabs[tabIndex].messages.append(msg)

          case .error(_, let message):
              tabs[tabIndex].isThinking = false
              let msg = ChatMessage(role: .system, content: "Error: \(message)")
              tabs[tabIndex].messages.append(msg)

          case .pong, .unknown:
              break
          }
      }
  }
  ```

  > **Note for new Swift developers:** `@Observable` (from the `Observation` framework, available macOS 14+) automatically tracks which properties a SwiftUI view reads and re-renders the view when those properties change. It replaces the older `ObservableObject` / `@Published` pattern.

- [ ] **Step 2: Build**

  ```bash
  cd residuum/mac
  xcodebuild -scheme Residuum -configuration Debug build 2>&1 | tail -3
  ```

  Expected: `** BUILD SUCCEEDED **`

  If you see a warning about the `turnStarted` case appearing twice in the switch: remove the second `case .turnStarted:` line at the bottom of `handle(_:tabIndex:)` — it's a copy-paste error in this plan.

- [ ] **Step 3: Commit**

  ```bash
  cd residuum
  git add mac/Residuum/Agents/AgentStore.swift
  git commit -m "feat(mac): add AgentStore with multi-agent WebSocket management"
  ```

---

## Task 8: App entry point with AgentStore injected

**Files:**
- Modify: `mac/Residuum/App/ResiduumApp.swift`
- Modify: `mac/Residuum/App/AppDelegate.swift`

- [ ] **Step 1: Update ResiduumApp.swift**

  Replace `mac/Residuum/App/ResiduumApp.swift`:

  ```swift
  import SwiftUI

  @main
  struct ResiduumApp: App {
      @NSApplicationDelegateAdaptor(AppDelegate.self) var appDelegate

      var body: some Scene {
          Settings { EmptyView() }
      }
  }
  ```

- [ ] **Step 2: Update AppDelegate.swift to inject AgentStore**

  Replace `mac/Residuum/App/AppDelegate.swift`:

  ```swift
  import AppKit
  import SwiftUI

  class AppDelegate: NSObject, NSApplicationDelegate {
      var statusItem: NSStatusItem!
      var popover: NSPopover!
      var expandedWindow: NSWindow?

      let store = AgentStore()

      func applicationDidFinishLaunching(_ notification: Notification) {
          setupStatusItem()
          setupPopover()
      }

      // MARK: - Status item

      private func setupStatusItem() {
          statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
          guard let button = statusItem.button else { return }
          button.image = NSImage(systemSymbolName: "circle.hexagonpath.fill",
                                 accessibilityDescription: "Residuum")
          button.image?.isTemplate = true
          button.action = #selector(togglePopover)
          button.target = self
      }

      // MARK: - Popover

      private func setupPopover() {
          popover = NSPopover()
          popover.contentSize = NSSize(width: 420, height: 520)
          popover.behavior = .transient
          let rootView = PopoverView(onExpand: { [weak self] in self?.openExpandedWindow() })
              .environment(store)
          popover.contentViewController = NSHostingController(rootView: rootView)
      }

      @objc func togglePopover() {
          guard let button = statusItem.button else { return }
          if popover.isShown {
              popover.performClose(nil)
          } else {
              popover.show(relativeTo: button.bounds, of: button, preferredEdge: .minY)
          }
      }

      // MARK: - Expanded window

      func openExpandedWindow() {
          popover.performClose(nil)

          if let window = expandedWindow {
              window.makeKeyAndOrderFront(nil)
              NSApp.activate(ignoringOtherApps: true)
              return
          }

          let rootView = PopoverView(onExpand: nil)
              .environment(store)
          let controller = NSHostingController(rootView: rootView)
          let window = NSWindow(contentViewController: controller)
          window.title = "Residuum"
          window.setContentSize(NSSize(width: 800, height: 600))
          window.styleMask = [.titled, .closable, .resizable, .miniaturizable]
          window.center()
          window.isReleasedWhenClosed = false
          window.delegate = self
          window.makeKeyAndOrderFront(nil)
          NSApp.activate(ignoringOtherApps: true)
          expandedWindow = window
      }
  }

  extension AppDelegate: NSWindowDelegate {
      func windowWillClose(_ notification: Notification) {
          expandedWindow = nil
      }
  }
  ```

- [ ] **Step 3: Build**

  ```bash
  cd residuum/mac
  xcodebuild -scheme Residuum -configuration Debug build 2>&1 | tail -3
  ```

  Expected: `** BUILD SUCCEEDED **` (will warn that `PopoverView` doesn't exist yet — that's fine, we'll add it next)

  If it errors on `PopoverView`, add a temporary stub in `mac/Residuum/Views/PopoverView.swift`:

  ```swift
  import SwiftUI
  struct PopoverView: View {
      var onExpand: (() -> Void)?
      var body: some View { Text("Coming soon") }
  }
  ```

- [ ] **Step 4: Commit**

  ```bash
  cd residuum
  git add mac/Residuum/App/
  git commit -m "feat(mac): wire app delegate with AgentStore and popover/window lifecycle"
  ```

---

## Task 9: Style constants

**Files:**
- Create: `mac/Residuum/Views/Style.swift`

All colours, fonts, and spacing in one place. Views import nothing extra — just reference `Style`.

- [ ] **Step 1: Write Style.swift**

  Create `mac/Residuum/Views/Style.swift`:

  ```swift
  import SwiftUI

  /// Single source of truth for the Residuum geological aesthetic.
  enum Style {
      // MARK: - Colours

      static let background    = Color(hex: "#0e0e10")
      static let surface       = Color(hex: "#111114")
      static let surfaceRaised = Color(hex: "#1a1a1d")
      static let border        = Color(hex: "#1e1e22")
      static let borderMid     = Color(hex: "#222226")
      static let blue          = Color(hex: "#3b8bdb")
      static let blueSubtle    = Color(hex: "#3b8bdb", opacity: 0.15)
      static let blueBorder    = Color(hex: "#3b8bdb", opacity: 0.25)
      static let moss          = Color(hex: "#6b7a4a")
      static let textPrimary   = Color(hex: "#c0c0c0")
      static let textMuted     = Color(hex: "#555555")
      static let textDim       = Color(hex: "#333333")
      static let userBubble    = Color(hex: "#1a2535")
      static let userBorder    = Color(hex: "#2a3a50")

      // MARK: - Fonts

      static func cinzel(size: CGFloat, weight: Font.Weight = .regular) -> Font {
          .custom("Cinzel", size: size).weight(weight)
      }

      static func literata(size: CGFloat) -> Font {
          .custom("Literata", size: size)
      }

      static func mono(size: CGFloat) -> Font {
          .custom("JetBrains Mono", size: size)
      }

      // MARK: - Spacing

      static let popoverWidth: CGFloat  = 420
      static let popoverHeight: CGFloat = 520
      static let windowWidth: CGFloat   = 800
      static let windowHeight: CGFloat  = 600
      static let headerHeight: CGFloat  = 44
      static let inputBarPad: CGFloat   = 10
  }

  // MARK: - Vein divider

  /// The luminescent blue vein that separates layout zones.
  struct VeinDivider: View {
      var body: some View {
          Rectangle()
              .fill(
                  LinearGradient(
                      colors: [.clear, Style.blue.opacity(0.25), .clear],
                      startPoint: .leading,
                      endPoint: .trailing
                  )
              )
              .frame(height: 1)
      }
  }

  // MARK: - Color(hex:) initialiser

  extension Color {
      init(hex: String, opacity: Double = 1) {
          let hex = hex.trimmingCharacters(in: CharacterSet.alphanumerics.inverted)
          var int: UInt64 = 0
          Scanner(string: hex).scanHexInt64(&int)
          let r = Double((int >> 16) & 0xFF) / 255
          let g = Double((int >> 8)  & 0xFF) / 255
          let b = Double(int & 0xFF)          / 255
          self.init(.sRGB, red: r, green: g, blue: b, opacity: opacity)
      }
  }
  ```

- [ ] **Step 2: Build**

  ```bash
  cd residuum/mac
  xcodebuild -scheme Residuum -configuration Debug build 2>&1 | tail -3
  ```

  Expected: `** BUILD SUCCEEDED **`

- [ ] **Step 3: Commit**

  ```bash
  cd residuum
  git add mac/Residuum/Views/Style.swift
  git commit -m "feat(mac): add Style constants (colours, fonts, vein divider)"
  ```

---

## Task 10: Install custom fonts

**Files:**
- Create: `mac/Residuum/Resources/Fonts/` (font files)

- [ ] **Step 1: Download the fonts**

  Download the following from Google Fonts and JetBrains:

  | Font | URL |
  |---|---|
  | Cinzel (Regular + SemiBold) | https://fonts.google.com/specimen/Cinzel → Download family |
  | Literata (Light + Light Italic) | https://fonts.google.com/specimen/Literata → Download family |
  | JetBrains Mono (Regular) | https://www.jetbrains.com/lp/mono/ → Download |

  Place these files in `mac/Residuum/Resources/Fonts/`:
  - `Cinzel-Regular.ttf`
  - `Cinzel-SemiBold.ttf`
  - `Literata[opsz,wght].ttf` (variable font — rename to `Literata-Light.ttf`)
  - `JetBrainsMono-Regular.ttf`

- [ ] **Step 2: Add fonts to the Xcode project via project.yml**

  Open `mac/project.yml` and add a `resources` key to the `Residuum` target so xcodegen copies the fonts into the app bundle:

  ```yaml
  targets:
    Residuum:
      type: application
      platform: macOS
      sources:
        - path: Residuum
      resources:
        - path: Residuum/Resources/Fonts
          buildPhase: resources
  ```

  Then regenerate:

  ```bash
  cd residuum/mac
  xcodegen generate
  ```

- [ ] **Step 3: Verify fonts load at runtime**

  Temporarily add this to `AppDelegate.applicationDidFinishLaunching`:

  ```swift
  // Debug: confirm fonts are registered
  print(NSFontManager.shared.availableFontFamilies.filter {
      $0.contains("Cinzel") || $0.contains("Literata") || $0.contains("JetBrains")
  })
  ```

  Run the app from Xcode (⌘R) and check the console output. You should see the three families listed. Remove the debug line after confirming.

- [ ] **Step 4: Commit**

  ```bash
  cd residuum
  git add mac/Residuum/Resources/Fonts/ mac/project.yml
  git commit -m "feat(mac): add Cinzel, Literata, JetBrains Mono fonts"
  ```

---

## Task 11: ThinkingIndicator

**Files:**
- Create: `mac/Residuum/Views/ThinkingIndicator.swift`

- [ ] **Step 1: Write ThinkingIndicator.swift**

  Create `mac/Residuum/Views/ThinkingIndicator.swift`:

  ```swift
  import SwiftUI

  /// Animated three-dot indicator shown while the agent is processing.
  struct ThinkingIndicator: View {
      @State private var phase = 0

      var body: some View {
          HStack(spacing: 4) {
              ForEach(0..<3, id: \.self) { i in
                  Circle()
                      .fill(Style.blue.opacity(phase == i ? 1 : 0.25))
                      .frame(width: 5, height: 5)
              }
              Text("thinking")
                  .font(Style.literata(size: 11))
                  .italic()
                  .foregroundStyle(Style.textMuted)
          }
          .onAppear {
              withAnimation(.linear(duration: 0.4).repeatForever()) {
                  // timer drives phase cycling
              }
              Timer.scheduledTimer(withTimeInterval: 0.4, repeats: true) { timer in
                  withAnimation(.easeInOut(duration: 0.2)) {
                      phase = (phase + 1) % 3
                  }
              }
          }
      }
  }
  ```

- [ ] **Step 2: Build**

  ```bash
  cd residuum/mac
  xcodebuild -scheme Residuum -configuration Debug build 2>&1 | tail -3
  ```

  Expected: `** BUILD SUCCEEDED **`

- [ ] **Step 3: Commit**

  ```bash
  cd residuum
  git add mac/Residuum/Views/ThinkingIndicator.swift
  git commit -m "feat(mac): add animated ThinkingIndicator"
  ```

---

## Task 12: ToolGroup — collapsible tool call visualization

**Files:**
- Create: `mac/Residuum/Views/ToolGroup.swift`

- [ ] **Step 1: Write ToolGroup.swift**

  Create `mac/Residuum/Views/ToolGroup.swift`:

  ```swift
  import SwiftUI

  /// A collapsible group showing all tool calls made during one assistant turn.
  struct ToolGroup: View {
      let toolCalls: [ToolCallData]
      @State private var expanded = false

      var body: some View {
          VStack(alignment: .leading, spacing: 0) {
              // Header row — always visible
              Button {
                  withAnimation(.easeInOut(duration: 0.2)) { expanded.toggle() }
              } label: {
                  HStack(spacing: 6) {
                      Image(systemName: expanded ? "chevron.down" : "chevron.right")
                          .font(.system(size: 9))
                          .foregroundStyle(Style.textDim)
                      Text("\(toolCalls.count) \(toolCalls.count == 1 ? "tool" : "tools") used")
                          .font(Style.mono(size: 10))
                          .foregroundStyle(Style.moss)
                      Spacer()
                  }
                  .padding(.horizontal, 10)
                  .padding(.vertical, 7)
              }
              .buttonStyle(.plain)

              // Expanded detail
              if expanded {
                  VStack(alignment: .leading, spacing: 6) {
                      ForEach(toolCalls) { call in
                          ToolCallRow(call: call)
                      }
                  }
                  .padding(.horizontal, 10)
                  .padding(.bottom, 8)
              }
          }
          .background(Style.surface)
          .clipShape(RoundedRectangle(cornerRadius: 6))
          .overlay(
              RoundedRectangle(cornerRadius: 6)
                  .stroke(Style.border, lineWidth: 1)
          )
      }
  }

  private struct ToolCallRow: View {
      let call: ToolCallData

      var body: some View {
          VStack(alignment: .leading, spacing: 2) {
              // Tool name
              Text(call.name)
                  .font(Style.mono(size: 10))
                  .foregroundStyle(Style.blue.opacity(0.7))

              // Arguments (one key: value per line)
              if !call.arguments.isEmpty {
                  Text(formatArguments(call.arguments))
                      .font(Style.mono(size: 9))
                      .foregroundStyle(Style.textMuted)
                      .lineLimit(3)
              }

              // Result
              if let result = call.result {
                  HStack(alignment: .top, spacing: 4) {
                      Rectangle()
                          .fill(call.isError ? Color.red.opacity(0.4) : Style.blue.opacity(0.2))
                          .frame(width: 2)
                      Text(result)
                          .font(Style.mono(size: 9))
                          .foregroundStyle(call.isError ? Color.red.opacity(0.7) : Style.textMuted)
                          .lineLimit(4)
                  }
              }
          }
          .padding(.leading, 8)
          .padding(.vertical, 2)
      }

      private func formatArguments(_ args: [String: JSONValue]) -> String {
          args.map { "\($0.key): \($0.value)" }.joined(separator: "\n")
      }
  }
  ```

- [ ] **Step 2: Build**

  ```bash
  cd residuum/mac
  xcodebuild -scheme Residuum -configuration Debug build 2>&1 | tail -3
  ```

- [ ] **Step 3: Commit**

  ```bash
  cd residuum
  git add mac/Residuum/Views/ToolGroup.swift
  git commit -m "feat(mac): add collapsible ToolGroup view"
  ```

---

## Task 13: MessageRow

**Files:**
- Create: `mac/Residuum/Views/MessageRow.swift`

- [ ] **Step 1: Write MessageRow.swift**

  Create `mac/Residuum/Views/MessageRow.swift`:

  ```swift
  import SwiftUI

  /// Renders a single `ChatMessage` in the chat feed.
  struct MessageRow: View {
      let message: ChatMessage

      var body: some View {
          switch message.role {
          case .user:      UserBubble(content: message.content)
          case .assistant: AssistantMessage(message: message)
          case .system:    SystemNotice(content: message.content)
          }
      }
  }

  // MARK: - User bubble

  private struct UserBubble: View {
      let content: String

      var body: some View {
          HStack {
              Spacer(minLength: 40)
              VStack(alignment: .trailing, spacing: 4) {
                  Text("you")
                      .font(Style.mono(size: 9))
                      .foregroundStyle(Style.textDim)
                      .textCase(.uppercase)
                  Text(content)
                      .font(Style.literata(size: 13))
                      .foregroundStyle(Color(hex: "#c8d8e8"))
                      .padding(.horizontal, 12)
                      .padding(.vertical, 8)
                      .background(Style.userBubble)
                      .clipShape(
                          UnevenRoundedRectangle(
                              topLeadingRadius: 12, bottomLeadingRadius: 12,
                              bottomTrailingRadius: 2, topTrailingRadius: 12
                          )
                      )
                      .overlay(
                          UnevenRoundedRectangle(
                              topLeadingRadius: 12, bottomLeadingRadius: 12,
                              bottomTrailingRadius: 2, topTrailingRadius: 12
                          )
                          .stroke(Style.userBorder, lineWidth: 1)
                      )
              }
          }
      }
  }

  // MARK: - Assistant message

  private struct AssistantMessage: View {
      let message: ChatMessage

      var body: some View {
          VStack(alignment: .leading, spacing: 6) {
              if !message.toolCalls.isEmpty {
                  ToolGroup(toolCalls: message.toolCalls)
              }
              if !message.content.isEmpty {
                  Text(message.content)
                      .font(Style.literata(size: 13))
                      .foregroundStyle(Style.textPrimary)
                      .lineSpacing(3)
                      .textSelection(.enabled)
              }
          }
      }
  }

  // MARK: - System notice

  private struct SystemNotice: View {
      let content: String

      var body: some View {
          Text(content)
              .font(Style.literata(size: 11))
              .italic()
              .foregroundStyle(Style.textMuted)
              .frame(maxWidth: .infinity)
              .multilineTextAlignment(.center)
      }
  }
  ```

  > **Note for new Swift developers:** `UnevenRoundedRectangle` (macOS 14+) lets you set different corner radii on each corner — used here to give the user bubble a "tail" effect on the bottom-right.

- [ ] **Step 2: Build**

  ```bash
  cd residuum/mac
  xcodebuild -scheme Residuum -configuration Debug build 2>&1 | tail -3
  ```

- [ ] **Step 3: Commit**

  ```bash
  cd residuum
  git add mac/Residuum/Views/MessageRow.swift
  git commit -m "feat(mac): add MessageRow (user bubble, assistant, system notice)"
  ```

---

## Task 14: ChatView

**Files:**
- Create: `mac/Residuum/Views/ChatView.swift`

- [ ] **Step 1: Write ChatView.swift**

  Create `mac/Residuum/Views/ChatView.swift`:

  ```swift
  import SwiftUI

  /// Scrollable list of messages for the currently selected agent.
  struct ChatView: View {
      @Environment(AgentStore.self) private var store

      var body: some View {
          ScrollViewReader { proxy in
              ScrollView {
                  LazyVStack(alignment: .leading, spacing: 16) {
                      if let tab = store.selectedTab {
                          ForEach(tab.messages) { message in
                              MessageRow(message: message)
                                  .id(message.id)
                          }
                          if tab.isThinking {
                              ThinkingIndicator()
                                  .id("thinking")
                                  .padding(.leading, 2)
                          }
                      }
                  }
                  .padding(.horizontal, 14)
                  .padding(.vertical, 12)
              }
              .background(Style.background)
              .onChange(of: store.selectedTab?.messages.count) { _, _ in
                  scrollToBottom(proxy: proxy)
              }
              .onChange(of: store.selectedTab?.isThinking) { _, _ in
                  scrollToBottom(proxy: proxy)
              }
          }
      }

      private func scrollToBottom(proxy: ScrollViewProxy) {
          withAnimation(.easeOut(duration: 0.2)) {
              if store.selectedTab?.isThinking == true {
                  proxy.scrollTo("thinking", anchor: .bottom)
              } else if let last = store.selectedTab?.messages.last {
                  proxy.scrollTo(last.id, anchor: .bottom)
              }
          }
      }
  }
  ```

- [ ] **Step 2: Build**

  ```bash
  cd residuum/mac
  xcodebuild -scheme Residuum -configuration Debug build 2>&1 | tail -3
  ```

- [ ] **Step 3: Commit**

  ```bash
  cd residuum
  git add mac/Residuum/Views/ChatView.swift
  git commit -m "feat(mac): add ChatView with auto-scroll"
  ```

---

## Task 15: TabBar

**Files:**
- Create: `mac/Residuum/Views/TabBar.swift`

- [ ] **Step 1: Write TabBar.swift**

  Create `mac/Residuum/Views/TabBar.swift`:

  ```swift
  import SwiftUI

  /// Pill-style agent tab switcher shown in the header.
  struct TabBar: View {
      @Environment(AgentStore.self) private var store

      var body: some View {
          ScrollView(.horizontal, showsIndicators: false) {
              HStack(spacing: 4) {
                  ForEach(store.tabs) { tab in
                      TabPill(
                          tab: tab,
                          isSelected: tab.id == store.selectedTabId
                      ) {
                          store.select(tab)
                      }
                  }
              }
          }
      }
  }

  private struct TabPill: View {
      let tab: AgentTab
      let isSelected: Bool
      let onTap: () -> Void

      private var isConnected: Bool {
          tab.connection.state == .connected
      }

      var body: some View {
          Button(action: onTap) {
              HStack(spacing: 5) {
                  // Connection state dot
                  Circle()
                      .fill(isConnected ? Style.blue : Style.textDim)
                      .frame(width: 4, height: 4)
                  Text(tab.name)
                      .font(Style.mono(size: 10))
                      .foregroundStyle(isSelected ? Style.textPrimary : Style.textMuted)
              }
              .padding(.horizontal, 10)
              .padding(.vertical, 4)
              .background(isSelected ? Style.surfaceRaised : Color.clear)
              .clipShape(Capsule())
              .overlay(Capsule().stroke(isSelected ? Style.border : Color.clear, lineWidth: 1))
          }
          .buttonStyle(.plain)
      }
  }
  ```

- [ ] **Step 2: Build**

  ```bash
  cd residuum/mac
  xcodebuild -scheme Residuum -configuration Debug build 2>&1 | tail -3
  ```

- [ ] **Step 3: Commit**

  ```bash
  cd residuum
  git add mac/Residuum/Views/TabBar.swift
  git commit -m "feat(mac): add pill-style TabBar for agent switching"
  ```

---

## Task 16: InputBar with file upload

**Files:**
- Create: `mac/Residuum/Views/InputBar.swift`

- [ ] **Step 1: Write InputBar.swift**

  Create `mac/Residuum/Views/InputBar.swift`:

  ```swift
  import SwiftUI
  import AppKit

  /// Text input, file attachment chips, and send button.
  struct InputBar: View {
      @Environment(AgentStore.self) private var store
      @State private var text = ""
      @State private var attachedImages: [AttachedImage] = []
      @FocusState private var focused: Bool

      private var canSend: Bool {
          let connected = store.selectedTab?.connection.state == .connected
          let notThinking = store.selectedTab?.isThinking == false
          let hasContent = !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
          return connected && notThinking && hasContent
      }

      var body: some View {
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
                  // Attach button
                  Button { pickFiles() } label: {
                      Image(systemName: "paperclip")
                          .font(.system(size: 14))
                          .foregroundStyle(Style.textMuted)
                  }
                  .buttonStyle(.plain)
                  .help("Attach an image")

                  // Text field
                  TextField("", text: $text, axis: .vertical)
                      .font(Style.literata(size: 13))
                      .foregroundStyle(Style.textPrimary)
                      .textFieldStyle(.plain)
                      .lineLimit(1...6)
                      .focused($focused)
                      .onSubmit { if canSend { sendMessage() } }
                      .placeholder(when: text.isEmpty) {
                          Text("Message \(store.selectedTab?.name ?? "agent")…")
                              .font(Style.literata(size: 13))
                              .foregroundStyle(Style.textMuted)
                      }

                  // Send button
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

      // MARK: - Actions

      private func sendMessage() {
          let content = text.trimmingCharacters(in: .whitespacesAndNewlines)
          guard !content.isEmpty else { return }
          let images = attachedImages.map { $0.imageData }
          store.sendMessage(content: content, images: images)
          text = ""
          attachedImages = []
      }

      private func pickFiles() {
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
              let img = AttachedImage(
                  filename: url.lastPathComponent,
                  imageData: ImageData(mediaType: mediaType, data: base64)
              )
              attachedImages.append(img)
          }
      }

      private func mediaType(for url: URL) -> String? {
          switch url.pathExtension.lowercased() {
          case "png":  return "image/png"
          case "jpg", "jpeg": return "image/jpeg"
          case "gif":  return "image/gif"
          case "webp": return "image/webp"
          case "bmp":  return "image/bmp"
          case "tiff", "tif": return "image/tiff"
          default: return nil
          }
      }
  }

  // MARK: - AttachedImage

  private struct AttachedImage: Identifiable {
      let id = UUID()
      let filename: String
      let imageData: ImageData
  }

  // MARK: - FileChip

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

  // MARK: - Placeholder modifier

  extension View {
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
  cd residuum/mac
  xcodebuild -scheme Residuum -configuration Debug build 2>&1 | tail -3
  ```

- [ ] **Step 3: Commit**

  ```bash
  cd residuum
  git add mac/Residuum/Views/InputBar.swift
  git commit -m "feat(mac): add InputBar with file upload and send"
  ```

---

## Task 17: SettingsView

**Files:**
- Create: `mac/Residuum/Views/SettingsView.swift`

Settings opens as a sheet. Only stores `host` in `UserDefaults`; ports come from the registry.

- [ ] **Step 1: Write SettingsView.swift**

  Create `mac/Residuum/Views/SettingsView.swift`:

  ```swift
  import SwiftUI

  /// Settings sheet — host configuration and connection status.
  struct SettingsView: View {
      @Environment(AgentStore.self) private var store
      @Environment(\.dismiss) private var dismiss

      @AppStorage("residuum.host") private var host = "127.0.0.1"
      @State private var editingHost = ""

      var body: some View {
          VStack(alignment: .leading, spacing: 0) {
              // Header
              HStack {
                  Text("SETTINGS")
                      .font(Style.cinzel(size: 11))
                      .foregroundStyle(Style.blue)
                      .kerning(3)
                  Spacer()
                  Button { dismiss() } label: {
                      Image(systemName: "xmark")
                          .font(.system(size: 11))
                          .foregroundStyle(Style.textMuted)
                  }
                  .buttonStyle(.plain)
              }
              .padding(.horizontal, 20)
              .padding(.top, 20)
              .padding(.bottom, 16)

              VeinDivider()

              ScrollView {
                  VStack(alignment: .leading, spacing: 24) {
                      // Connection section
                      VStack(alignment: .leading, spacing: 12) {
                          Text("CONNECTION")
                              .font(Style.mono(size: 10))
                              .foregroundStyle(Style.textMuted)
                              .kerning(1)

                          // Host
                          VStack(alignment: .leading, spacing: 6) {
                              Text("Host")
                                  .font(Style.literata(size: 12))
                                  .foregroundStyle(Style.textMuted)
                              TextField("127.0.0.1", text: $editingHost)
                                  .font(Style.mono(size: 12))
                                  .foregroundStyle(Style.textPrimary)
                                  .textFieldStyle(.plain)
                                  .padding(.horizontal, 10)
                                  .padding(.vertical, 6)
                                  .background(Style.surface)
                                  .clipShape(RoundedRectangle(cornerRadius: 6))
                                  .overlay(RoundedRectangle(cornerRadius: 6)
                                      .stroke(Style.border, lineWidth: 1))
                              Text("Ports are read from the agent registry.")
                                  .font(Style.literata(size: 11))
                                  .italic()
                                  .foregroundStyle(Style.textDim)
                          }

                          // Agent status
                          VStack(alignment: .leading, spacing: 8) {
                              Text("Agents")
                                  .font(Style.literata(size: 12))
                                  .foregroundStyle(Style.textMuted)
                              ForEach(store.tabs) { tab in
                                  AgentStatusRow(tab: tab)
                              }
                          }
                      }
                  }
                  .padding(20)
              }

              VeinDivider()

              // Save button
              HStack {
                  Spacer()
                  Button("Save") {
                      host = editingHost
                      dismiss()
                  }
                  .font(Style.mono(size: 11))
                  .foregroundStyle(Style.blue)
                  .buttonStyle(.plain)
              }
              .padding(16)
          }
          .background(Style.background)
          .frame(width: 340, height: 400)
          .onAppear { editingHost = host }
      }
  }

  private struct AgentStatusRow: View {
      let tab: AgentTab

      private var stateLabel: String {
          switch tab.connection.state {
          case .connected:    return "connected"
          case .connecting:   return "connecting…"
          case .disconnected: return "disconnected"
          }
      }

      private var stateColor: Color {
          switch tab.connection.state {
          case .connected:    return Style.blue
          case .connecting:   return Style.moss
          case .disconnected: return Style.textDim
          }
      }

      var body: some View {
          HStack {
              Circle()
                  .fill(stateColor)
                  .frame(width: 5, height: 5)
              Text(tab.name)
                  .font(Style.mono(size: 11))
                  .foregroundStyle(Style.textPrimary)
              Spacer()
              Text(":\(tab.port)")
                  .font(Style.mono(size: 10))
                  .foregroundStyle(Style.textMuted)
              Text(stateLabel)
                  .font(Style.mono(size: 10))
                  .foregroundStyle(stateColor)
          }
      }
  }
  ```

- [ ] **Step 2: Build**

  ```bash
  cd residuum/mac
  xcodebuild -scheme Residuum -configuration Debug build 2>&1 | tail -3
  ```

- [ ] **Step 3: Commit**

  ```bash
  cd residuum
  git add mac/Residuum/Views/SettingsView.swift
  git commit -m "feat(mac): add SettingsView with host config and agent status"
  ```

---

## Task 18: PopoverView — assemble everything

**Files:**
- Modify (or replace): `mac/Residuum/Views/PopoverView.swift`

- [ ] **Step 1: Write PopoverView.swift**

  Replace (or create) `mac/Residuum/Views/PopoverView.swift`:

  ```swift
  import SwiftUI

  /// Root view rendered inside the NSPopover and the expanded NSWindow.
  struct PopoverView: View {
      @Environment(AgentStore.self) private var store
      /// Nil when shown inside the expanded window (button hidden).
      var onExpand: (() -> Void)?

      @State private var showSettings = false

      var body: some View {
          VStack(spacing: 0) {
              header
              VeinDivider()

              if store.selectedTab?.connection.state == .disconnected
                  && store.selectedTab?.messages.isEmpty == true {
                  disconnectedBody
              } else {
                  ChatView()
                  VeinDivider()
                  InputBar()
              }

              if onExpand != nil {
                  expandButton
              }
          }
          .background(Style.background)
          .sheet(isPresented: $showSettings) {
              SettingsView()
                  .environment(store)
          }
      }

      // MARK: - Header

      private var header: some View {
          HStack(spacing: 8) {
              // Wordmark
              Text("RESIDUUM")
                  .font(Style.cinzel(size: 11))
                  .foregroundStyle(Style.blue)
                  .kerning(3)

              // Tab bar
              TabBar()
                  .frame(maxWidth: .infinity)

              // Settings gear
              Button { showSettings = true } label: {
                  Image(systemName: "gearshape")
                      .font(.system(size: 13))
                      .foregroundStyle(Style.textMuted)
              }
              .buttonStyle(.plain)
              .help("Settings")
          }
          .padding(.horizontal, 14)
          .padding(.vertical, 10)
          .frame(height: Style.headerHeight)
      }

      // MARK: - Disconnected body

      private var disconnectedBody: some View {
          VStack(spacing: 16) {
              Spacer()
              Text("RESIDUUM")
                  .font(Style.cinzel(size: 13))
                  .foregroundStyle(Style.textDim)
                  .kerning(3)
              VStack(spacing: 6) {
                  Text("Daemon not running.")
                      .font(Style.mono(size: 11))
                      .foregroundStyle(Style.textMuted)
                  Text("residuum serve")
                      .font(Style.mono(size: 11))
                      .foregroundStyle(Style.blue.opacity(0.5))
              }
              Button("Reconnect") {
                  if let tab = store.selectedTab {
                      store.reconnect(tab: tab)
                  }
              }
              .font(Style.mono(size: 10))
              .foregroundStyle(Style.blue)
              .padding(.horizontal, 16)
              .padding(.vertical, 6)
              .overlay(RoundedRectangle(cornerRadius: 4).stroke(Style.blue.opacity(0.3)))
              .buttonStyle(.plain)
              Spacer()
          }
          .frame(maxWidth: .infinity, maxHeight: .infinity)
      }

      // MARK: - Expand button

      private var expandButton: some View {
          HStack {
              Spacer()
              Button {
                  onExpand?()
              } label: {
                  HStack(spacing: 4) {
                      Image(systemName: "arrow.up.left.and.arrow.down.right")
                          .font(.system(size: 9))
                      Text("open in window")
                          .font(Style.mono(size: 9))
                          .kerning(1)
                  }
                  .foregroundStyle(Style.textDim)
              }
              .buttonStyle(.plain)
              .help("Open in a detached window")
              .padding(.trailing, 12)
              .padding(.bottom, 6)
          }
          .onHover { inside in /* hover colour handled by button style */ }
      }
  }
  ```

- [ ] **Step 2: Build**

  ```bash
  cd residuum/mac
  xcodebuild -scheme Residuum -configuration Debug build 2>&1 | tail -3
  ```

  Expected: `** BUILD SUCCEEDED **`

- [ ] **Step 3: Run the app**

  Open `mac/Residuum.xcodeproj` in Xcode and press ⌘R. You should see:
  - A menu bar icon (`circle.hexagonpath.fill`)
  - Clicking it opens the popover
  - The popover shows the tab bar and either a chat view or disconnected state

- [ ] **Step 4: Commit**

  ```bash
  cd residuum
  git add mac/Residuum/Views/PopoverView.swift
  git commit -m "feat(mac): assemble PopoverView — full app UI wired up"
  ```

---

## Task 19: Menu bar icon disconnected state

**Files:**
- Modify: `mac/Residuum/App/AppDelegate.swift`
- Modify: `mac/Residuum/Agents/AgentStore.swift`

The menu bar icon should dim when the default agent is disconnected.

- [ ] **Step 1: Expose a computed property on AgentStore**

  Add to `AgentStore`:

  ```swift
  /// True if the default agent (first tab) is connected.
  var defaultAgentConnected: Bool {
      tabs.first?.connection.state == .connected
  }
  ```

- [ ] **Step 2: Update the status item in AppDelegate**

  Add a `stateTimer` property and an `updateStatusIcon` method. The timer polls every second — simple and reliable from AppDelegate context.

  Add to `AppDelegate`:

  ```swift
  private var stateTimer: Timer?

  private func startStatePolling() {
      stateTimer = Timer.scheduledTimer(withTimeInterval: 1.0, repeats: true) { [weak self] _ in
          self?.updateStatusIcon()
      }
  }

  private func updateStatusIcon() {
      let connected = store.defaultAgentConnected
      statusItem.button?.alphaValue = connected ? 1.0 : 0.35
  }
  ```

  And in `applicationDidFinishLaunching`, call it after `setupPopover()`:

  ```swift
  setupStatusItem()
  setupPopover()
  startStatePolling()
  ```

- [ ] **Step 3: Build and verify**

  ```bash
  cd residuum/mac
  xcodebuild -scheme Residuum -configuration Debug build 2>&1 | tail -3
  ```

  Run the app with the daemon stopped. The menu bar icon should appear dimmed. Start `residuum serve` and watch it brighten.

- [ ] **Step 4: Commit**

  ```bash
  cd residuum
  git add mac/Residuum/App/AppDelegate.swift mac/Residuum/Agents/AgentStore.swift
  git commit -m "feat(mac): dim status bar icon when daemon disconnected"
  ```

---

## Task 20: Add .gitignore entries and final build

**Files:**
- Modify: `residuum/.gitignore`
- Modify: `mac/project.yml` (add xcodeproj to gitignore or track it)

- [ ] **Step 1: Decide what to track in git**

  The generated `.xcodeproj` file can either be committed (simpler) or regenerated with `xcodegen generate` on each checkout. Committing it is easier for a single developer. Add the user data to gitignore but track the project file:

  Add to `residuum/.gitignore`:

  ```
  mac/Residuum.xcodeproj/project.xcworkspace/
  mac/Residuum.xcodeproj/xcuserdata/
  mac/*.xcworkspace/
  mac/DerivedData/
  ```

- [ ] **Step 2: Run the full test suite**

  ```bash
  cd residuum/mac
  xcodebuild test -scheme Residuum -destination 'platform=macOS' 2>&1 | grep -E "Test Suite|passed|failed"
  ```

  Expected: all tests pass (`ProtocolTests`, `AgentRegistryTests`).

- [ ] **Step 3: Commit everything**

  ```bash
  cd residuum
  git add .gitignore mac/
  git commit -m "feat(mac): complete Residuum Mac app v1"
  ```

---

## Done

At this point you have a working Residuum Mac app with:

- Menu bar icon that dims when the daemon is disconnected
- Popover with tabbed agent switching, chat feed, file upload, and settings
- Expand-to-window button
- Auto-reconnecting WebSocket per agent
- Full codec test coverage for the protocol

**To run:** `residuum serve` in a terminal, then launch the Mac app from Xcode (⌘R) or the built `.app`.
