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
}
