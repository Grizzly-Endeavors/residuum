import SwiftUI

@main
struct ResiduumApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) var appDelegate

    var body: some Scene {
        // No windows — this app lives entirely in the menu bar.
        // The Settings scene is required to suppress the "no scenes" warning.
        Settings { EmptyView() }
    }
}
