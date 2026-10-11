import SwiftUI

/// Settings sheet — hub host and agent selection.
struct SettingsView: View {
    @Environment(AgentStore.self) private var store
    @Environment(\.dismiss) private var dismiss

    @AppStorage("residuum.host") private var host = "127.0.0.1"
    @AppStorage("residuum.agent") private var agent = ""
    @State private var editingHost = ""
    @State private var editingAgent = ""

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
                        }

                        VStack(alignment: .leading, spacing: 6) {
                            Text("Agent")
                                .font(Style.literata(size: 12))
                                .foregroundStyle(Style.textMuted)
                            TextField("first running agent", text: $editingAgent)
                                .font(Style.mono(size: 12))
                                .foregroundStyle(Style.textPrimary)
                                .textFieldStyle(.plain)
                                .padding(.horizontal, 10)
                                .padding(.vertical, 6)
                                .background(Style.surface)
                                .clipShape(RoundedRectangle(cornerRadius: 6))
                                .overlay(RoundedRectangle(cornerRadius: 6)
                                    .stroke(Style.border, lineWidth: 1))
                            Text("Leave empty to use the first running agent.")
                                .font(Style.literata(size: 11))
                                .italic()
                                .foregroundStyle(Style.textDim)
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
                    agent = editingAgent
                    store.reconnect(host: editingHost, agent: editingAgent)
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
        .onAppear {
            editingHost = host
            editingAgent = agent
        }
    }
}
