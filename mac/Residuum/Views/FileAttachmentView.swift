import SwiftUI

/// Renders a file attachment from the daemon: inline image, audio link, or download link.
struct FileAttachmentView: View {
    let attachment: FileAttachmentData

    var body: some View {
        if attachment.mimeType.hasPrefix("image/"), let url = URL(string: attachment.url) {
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
        } else if let url = URL(string: attachment.url) {
            let icon = attachment.mimeType.hasPrefix("audio/") ? "play.circle" : "doc.arrow.down"
            Link(destination: url) {
                Label("\(attachment.filename) (\(formatSize(attachment.size)))", systemImage: icon)
                    .font(Style.literata(size: 13))
                    .foregroundStyle(Style.blue)
            }
            .buttonStyle(.plain)
        }
    }

    private func formatSize(_ bytes: Int) -> String {
        if bytes < 1024 { return "\(bytes) B" }
        if bytes < 1024 * 1024 { return "\(bytes / 1024) KB" }
        return "\(bytes / (1024 * 1024)) MB"
    }
}
