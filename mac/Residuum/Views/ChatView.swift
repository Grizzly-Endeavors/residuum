import SwiftUI

/// Scrollable list of messages for the connected agent.
struct ChatView: View {
    @Environment(AgentStore.self) private var store

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 16) {
                    ForEach(store.messages) { message in
                        MessageRow(message: message)
                            .id(message.id)
                    }
                    if store.isThinking {
                        ThinkingIndicator()
                            .id("thinking")
                            .padding(.leading, 2)
                    }
                }
                .padding(.horizontal, 14)
                .padding(.vertical, 12)
            }
            .background(Style.background)
            .onChange(of: store.messages.count) { _, _ in
                scrollToBottom(proxy: proxy)
            }
            .onChange(of: store.isThinking) { _, _ in
                scrollToBottom(proxy: proxy)
            }
        }
    }

    private func scrollToBottom(proxy: ScrollViewProxy) {
        withAnimation(.easeOut(duration: 0.2)) {
            if store.isThinking {
                proxy.scrollTo("thinking", anchor: .bottom)
            } else if let last = store.messages.last {
                proxy.scrollTo(last.id, anchor: .bottom)
            }
        }
    }
}
