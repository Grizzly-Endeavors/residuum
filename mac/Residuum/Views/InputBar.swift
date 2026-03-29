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
                    .onSubmit { if canSend { sendMessage() } }
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

    private func sendMessage() {
        let content = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !content.isEmpty else { return }
        let images = attachedImages.map { $0.imageData }
        store.sendMessage(content: content, images: images)
        text = ""
        attachedImages = []
    }

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
