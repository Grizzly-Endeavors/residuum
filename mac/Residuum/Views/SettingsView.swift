import SwiftUI

/// Settings sheet — host configuration and per-agent connection status.
struct SettingsView: View {
    @Environment(AgentStore.self) private var store
    @Environment(\.dismiss) private var dismiss

    @AppStorage("residuum.host") private var host = "127.0.0.1"
    @State private var editingHost = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
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
                    VStack(alignment: .leading, spacing: 12) {
                        Text("CONNECTION")
                            .font(Style.mono(size: 10))
                            .foregroundStyle(Style.textMuted)
                            .kerning(1)

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

            HStack {
                Spacer()
                Button("Save") {
                    host = editingHost
                    store.reconnectAll(host: editingHost)
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
