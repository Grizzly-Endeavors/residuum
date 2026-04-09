import AVKit
import SwiftUI

/// Renders a file attachment from the daemon: inline image, audio player, or download button.
struct FileAttachmentView: View {
    let attachment: FileAttachmentData

    private var resolvedURL: URL? {
        URL(string: attachment.url)
    }

    var body: some View {
        Group {
            if let url = resolvedURL {
                if attachment.mimeType.hasPrefix("image/") {
                    AsyncImage(url: url) { phase in
                        switch phase {
                        case .success(let image):
                            image.resizable().scaledToFit()
                        case .failure:
                            Label(attachment.filename, systemImage: "photo")
                                .foregroundStyle(Style.textMuted)
                        default:
                            ProgressView()
                        }
                    }
                    .frame(maxWidth: 300)
                } else if attachment.mimeType.hasPrefix("audio/") {
                    AudioPlayerView(url: url, filename: attachment.filename)
                } else {
                    DownloadButton(url: url, filename: attachment.filename, size: attachment.size)
                }
            } else {
                Text("[\(attachment.filename)]")
                    .font(Style.literata(size: 13))
                    .foregroundStyle(Style.textMuted)
            }
        }
    }
}

/// Inline audio player using AVPlayer.
private struct AudioPlayerView: View {
    let url: URL
    let filename: String
    @State private var player: AVPlayer?
    @State private var isPlaying = false

    var body: some View {
        HStack(spacing: 8) {
            Button {
                if isPlaying {
                    player?.pause()
                } else {
                    if player == nil {
                        player = AVPlayer(url: url)
                    }
                    player?.play()
                }
                isPlaying.toggle()
            } label: {
                Image(systemName: isPlaying ? "pause.circle.fill" : "play.circle.fill")
                    .font(.system(size: 24))
                    .foregroundStyle(Style.blue)
            }
            .buttonStyle(.plain)

            Text(filename)
                .font(Style.literata(size: 13))
                .foregroundStyle(Style.textPrimary)
        }
        .onDisappear {
            player?.pause()
            player = nil
        }
    }
}

/// Download button that presents a save panel.
private struct DownloadButton: View {
    let url: URL
    let filename: String
    let size: Int

    var body: some View {
        Button {
            let panel = NSSavePanel()
            panel.nameFieldStringValue = filename
            panel.canCreateDirectories = true
            if panel.runModal() == .OK, let dest = panel.url {
                URLSession.shared.downloadTask(with: url) { tempURL, _, error in
                    guard let tempURL, error == nil else { return }
                    try? FileManager.default.moveItem(at: tempURL, to: dest)
                }.resume()
            }
        } label: {
            Label("\(filename) (\(formatSize(size)))", systemImage: "doc.arrow.down")
                .font(Style.literata(size: 13))
                .foregroundStyle(Style.blue)
        }
        .buttonStyle(.plain)
    }

    private func formatSize(_ bytes: Int) -> String {
        if bytes < 1024 { return "\(bytes) B" }
        if bytes < 1024 * 1024 { return "\(bytes / 1024) KB" }
        return "\(bytes / (1024 * 1024)) MB"
    }
}
