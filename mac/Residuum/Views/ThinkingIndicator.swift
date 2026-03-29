import SwiftUI

/// Animated three-dot indicator shown while the agent is processing.
struct ThinkingIndicator: View {
    @State private var phase = 0

    var body: some View {
        HStack(spacing: 4) {
            ForEach(0..<3, id: \.self) { i in
                Circle()
                    .fill(Style.blue.opacity(phase == i ? 1 : 0.25))
                    .frame(width: 5, height: 5)
            }
            Text("thinking")
                .font(Style.literata(size: 11))
                .italic()
                .foregroundStyle(Style.textMuted)
        }
        .onAppear {
            Timer.scheduledTimer(withTimeInterval: 0.4, repeats: true) { _ in
                withAnimation(.easeInOut(duration: 0.2)) {
                    phase = (phase + 1) % 3
                }
            }
        }
    }
}
