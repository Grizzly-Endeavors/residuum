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
