import AppKit
import SwiftUI

class AppDelegate: NSObject, NSApplicationDelegate {
    var statusItem: NSStatusItem!
    var popover: NSPopover!
    var expandedWindow: NSWindow?
    private var stateTimer: Timer?

    let store = AgentStore()

    func applicationDidFinishLaunching(_ notification: Notification) {
        setupStatusItem()
        setupPopover()
        startStatePolling()
    }

    // MARK: - Status item

    private func setupStatusItem() {
        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        guard let button = statusItem.button else { return }
        button.image = NSImage(systemSymbolName: "circle.hexagonpath.fill",
                               accessibilityDescription: "Residuum")
        button.image?.isTemplate = true
        button.action = #selector(togglePopover)
        button.target = self
    }

    // MARK: - Popover

    private func setupPopover() {
        popover = NSPopover()
        popover.contentSize = NSSize(width: 420, height: 520)
        popover.behavior = .transient
        let rootView = PopoverView(onExpand: { [weak self] in self?.openExpandedWindow() })
            .environment(store)
        popover.contentViewController = NSHostingController(rootView: rootView)
    }

    @objc func togglePopover() {
        guard let button = statusItem.button else { return }
        if popover.isShown {
            popover.performClose(nil)
        } else {
            popover.show(relativeTo: button.bounds, of: button, preferredEdge: .minY)
        }
    }

    // MARK: - Expanded window

    func openExpandedWindow() {
        popover.performClose(nil)

        // Temporarily become a regular app so the window can take focus properly.
        // Without this, LSUIElement apps freeze on window presentation in macOS 13+.
        NSApp.setActivationPolicy(.regular)

        if let window = expandedWindow {
            window.makeKeyAndOrderFront(nil)
            NSApp.activate(ignoringOtherApps: true)
            return
        }

        let rootView = PopoverView(onExpand: nil)
            .environment(store)
        let controller = NSHostingController(rootView: rootView)
        let window = NSWindow(contentViewController: controller)
        window.title = "Residuum Chat"
        window.setContentSize(NSSize(width: 800, height: 600))
        window.styleMask = [.titled, .closable, .resizable, .miniaturizable]
        window.center()
        window.isReleasedWhenClosed = false
        window.delegate = self
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        expandedWindow = window
    }

    // MARK: - Icon state

    private func startStatePolling() {
        stateTimer = Timer.scheduledTimer(withTimeInterval: 1.0, repeats: true) { [weak self] _ in
            self?.updateStatusIcon()
        }
    }

    private func updateStatusIcon() {
        statusItem.button?.alphaValue = store.defaultAgentConnected ? 1.0 : 0.35
    }
}

extension AppDelegate: NSWindowDelegate {
    func windowWillClose(_ notification: Notification) {
        expandedWindow = nil
        // Return to accessory mode so we disappear from the Dock again.
        NSApp.setActivationPolicy(.accessory)
    }
}
