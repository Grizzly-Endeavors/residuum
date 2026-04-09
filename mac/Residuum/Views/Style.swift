import SwiftUI

/// Single source of truth for the Residuum geological aesthetic.
enum Style {
    // MARK: - Colours

    static let background    = Color(hex: "#0e0e10")
    static let surface       = Color(hex: "#111114")
    static let surfaceRaised = Color(hex: "#1a1a1d")
    static let border        = Color(hex: "#1e1e22")
    static let borderMid     = Color(hex: "#222226")
    static let blue          = Color(hex: "#3b8bdb")
    static let blueSubtle    = Color(hex: "#3b8bdb", opacity: 0.15)
    static let blueBorder    = Color(hex: "#3b8bdb", opacity: 0.25)
    static let moss          = Color(hex: "#6b7a4a")
    static let textPrimary   = Color(hex: "#c0c0c0")
    static let textMuted     = Color(hex: "#555555")
    static let textDim       = Color(hex: "#333333")
    static let userBubble    = Color(hex: "#1a2535")
    static let userBorder    = Color(hex: "#2a3a50")

    // MARK: - Fonts

    /// Cinzel serif — used for headings and the wordmark.
    static func cinzel(size: CGFloat, weight: Font.Weight = .regular) -> Font {
        .custom("Cinzel", size: size).weight(weight)
    }

    /// Literata light — used for body text and conversation content.
    static func literata(size: CGFloat) -> Font {
        .custom("Literata", size: size)
    }

    /// JetBrains Mono — used for code, labels, and technical elements.
    static func mono(size: CGFloat) -> Font {
        .custom("JetBrains Mono", size: size)
    }

    // MARK: - Spacing

    static let popoverWidth: CGFloat  = 420
    static let popoverHeight: CGFloat = 520
    static let windowWidth: CGFloat   = 800
    static let windowHeight: CGFloat  = 600
    static let headerHeight: CGFloat  = 44
    static let inputBarPad: CGFloat   = 10
}

// MARK: - Vein divider

/// The luminescent blue vein that separates layout zones.
struct VeinDivider: View {
    var body: some View {
        Rectangle()
            .fill(
                LinearGradient(
                    colors: [.clear, Style.blue.opacity(0.25), .clear],
                    startPoint: .leading,
                    endPoint: .trailing
                )
            )
            .frame(height: 1)
    }
}

// MARK: - Color(hex:) initialiser

extension Color {
    /// Initialise from a hex string, e.g. `"#0e0e10"` or `"0e0e10"`.
    init(hex: String, opacity: Double = 1) {
        let hex = hex.trimmingCharacters(in: CharacterSet.alphanumerics.inverted)
        var int: UInt64 = 0
        Scanner(string: hex).scanHexInt64(&int)
        let r = Double((int >> 16) & 0xFF) / 255
        let g = Double((int >> 8)  & 0xFF) / 255
        let b = Double(int & 0xFF)          / 255
        self.init(.sRGB, red: r, green: g, blue: b, opacity: opacity)
    }
}
