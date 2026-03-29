import SwiftUI

/// Root view rendered inside the NSPopover and the expanded NSWindow.
struct PopoverView: View {
    @Environment(AgentStore.self) private var store
    /// Nil when shown inside the expanded window (expand button hidden).
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
            Text("RESIDUUM")
                .font(Style.cinzel(size: 11))
                .foregroundStyle(Style.blue)
                .kerning(3)

            TabBar()
                .frame(maxWidth: .infinity)

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
    }
}
