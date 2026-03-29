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
