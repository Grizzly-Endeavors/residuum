import SwiftUI

/// The autocomplete menu shown above the input bar when the user types `/`.
///
/// Rendered as a list of `CommandMenuItem` rows separated by vein dividers.
/// The parent (`InputBar`) owns the selection index and calls `onSelect` on tap or keyboard Enter.
struct CommandMenu: View {
    let commands: [SlashCommand]
    let selectedIndex: Int
    let onSelect: (SlashCommand) -> Void

    var body: some View {
        VStack(spacing: 0) {
            VeinDivider()
            ForEach(Array(commands.enumerated()), id: \.element.id) { index, cmd in
                CommandMenuItem(
                    command: cmd,
                    isSelected: index == selectedIndex
                )
                .onTapGesture { onSelect(cmd) }
            }
            VeinDivider()
        }
        .background(Style.background)
    }
}

/// A single row in the command menu.
private struct CommandMenuItem: View {
    let command: SlashCommand
    let isSelected: Bool

    var body: some View {
        HStack(spacing: 0) {
            Text(command.name)
                .font(Style.mono(size: 11))
                .foregroundStyle(isSelected ? Style.blue : Style.textMuted)
                .frame(width: 90, alignment: .leading) // wide enough for all current command names
            Text(command.description)
                .font(Style.literata(size: 11))
                .italic()
                .foregroundStyle(isSelected ? Style.textMuted : Style.textDim)
            Spacer()
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 6)
        .background(isSelected ? Style.surfaceRaised : Color.clear)
    }
}
