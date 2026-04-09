import SwiftUI

/// A collapsible group showing all tool calls made during one assistant turn.
struct ToolGroup: View {
    let toolCalls: [ToolCallData]
    @State private var expanded = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            // Header row — always visible
            Button {
                withAnimation(.easeInOut(duration: 0.2)) { expanded.toggle() }
            } label: {
                HStack(spacing: 6) {
                    Image(systemName: expanded ? "chevron.down" : "chevron.right")
                        .font(.system(size: 9))
                        .foregroundStyle(Style.textDim)
                    Text("\(toolCalls.count) \(toolCalls.count == 1 ? "tool" : "tools") used")
                        .font(Style.mono(size: 10))
                        .foregroundStyle(Style.moss)
                    Spacer()
                }
                .padding(.horizontal, 10)
                .padding(.vertical, 7)
            }
            .buttonStyle(.plain)

            // Expanded detail
            if expanded {
                VStack(alignment: .leading, spacing: 6) {
                    ForEach(toolCalls) { call in
                        ToolCallRow(call: call)
                    }
                }
                .padding(.horizontal, 10)
                .padding(.bottom, 8)
            }
        }
        .background(Style.surface)
        .clipShape(RoundedRectangle(cornerRadius: 6))
        .overlay(
            RoundedRectangle(cornerRadius: 6)
                .stroke(Style.border, lineWidth: 1)
        )
    }
}

private struct ToolCallRow: View {
    let call: ToolCallData

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(call.name)
                .font(Style.mono(size: 10))
                .foregroundStyle(Style.blue.opacity(0.7))

            if !call.arguments.isEmpty {
                Text(formatArguments(call.arguments))
                    .font(Style.mono(size: 9))
                    .foregroundStyle(Style.textMuted)
                    .lineLimit(3)
            }

            if let result = call.result {
                HStack(alignment: .top, spacing: 4) {
                    Rectangle()
                        .fill(call.isError ? Color.red.opacity(0.4) : Style.blue.opacity(0.2))
                        .frame(width: 2)
                    Text(result)
                        .font(Style.mono(size: 9))
                        .foregroundStyle(call.isError ? Color.red.opacity(0.7) : Style.textMuted)
                        .lineLimit(4)
                }
            }
        }
        .padding(.leading, 8)
        .padding(.vertical, 2)
    }

    private func formatArguments(_ args: [String: JSONValue]) -> String {
        args.map { "\($0.key): \($0.value)" }.joined(separator: "\n")
    }
}
