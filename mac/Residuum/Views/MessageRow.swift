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
