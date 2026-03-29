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
