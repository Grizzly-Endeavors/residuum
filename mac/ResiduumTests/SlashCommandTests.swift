import XCTest
@testable import Residuum

final class SlashCommandTests: XCTestCase {

    // MARK: - Registry shape

    func testRegistryHasEightCommands() {
        XCTAssertEqual(COMMAND_REGISTRY.count, 8)
    }

    func testOnlyInboxHasArgs() {
        let withArgs = COMMAND_REGISTRY.filter { $0.hasArgs }
        XCTAssertEqual(withArgs.count, 1)
        XCTAssertEqual(withArgs.first?.name, "/inbox")
    }

    func testAllCommandsStartWithSlash() {
        for cmd in COMMAND_REGISTRY {
            XCTAssertTrue(cmd.name.hasPrefix("/"),
                "\(cmd.name) must start with /")
        }
    }

    func testRegistryContainsExpectedCommands() {
        let names = COMMAND_REGISTRY.map { $0.name }
        XCTAssertEqual(names, [
            "/help", "/verbose", "/status",
            "/observe", "/reflect", "/context",
            "/reload", "/inbox"
        ])
    }

    // MARK: - Filtering

    func testEmptyQueryReturnsAll() {
        // Empty query uses hasPrefix("/") — all commands start with /, so all 8 match.
        let query = ""
        let filtered = COMMAND_REGISTRY.filter { $0.name.hasPrefix("/" + query) }
        XCTAssertEqual(filtered.count, 8)
    }

    func testPrefixFilterMatchesSingle() {
        let filtered = COMMAND_REGISTRY.filter { $0.name.hasPrefix("/ob") }
        XCTAssertEqual(filtered.count, 1)
        XCTAssertEqual(filtered.first?.name, "/observe")
    }

    func testPrefixFilterMatchesMultiple() {
        let filtered = COMMAND_REGISTRY.filter { $0.name.hasPrefix("/re") }
        XCTAssertEqual(filtered.count, 2) // /reflect, /reload
        XCTAssertEqual(filtered.map { $0.name }, ["/reflect", "/reload"])
    }

    func testPrefixFilterNoMatch() {
        let filtered = COMMAND_REGISTRY.filter { $0.name.hasPrefix("/zzz") }
        XCTAssertTrue(filtered.isEmpty)
    }

    // MARK: - AgentStore system messages

    func testAppendSystemMessageAddsToSelectedTab() {
        let store = AgentStore()
        let initial = store.selectedTab?.messages.count ?? 0
        store.appendSystemMessage("hello notice")
        XCTAssertEqual(store.selectedTab?.messages.count, initial + 1)
        let last = store.selectedTab?.messages.last
        XCTAssertEqual(last?.content, "hello notice")
        XCTAssertEqual(last?.role, .system)
    }

    func testAppendSystemBlockAddsSystemBlockRole() {
        let store = AgentStore()
        store.appendSystemBlock("key   value")
        let last = store.selectedTab?.messages.last
        XCTAssertEqual(last?.role, .systemBlock)
        XCTAssertEqual(last?.content, "key   value")
    }

    func testAppendDoesNothingWhenNoTabsSelected() {
        let store = AgentStore()
        store.tabs.removeAll()
        // Must not crash — no assertion needed beyond absence of crash
        store.appendSystemMessage("orphan")
        store.appendSystemBlock("orphan block")
    }
}
