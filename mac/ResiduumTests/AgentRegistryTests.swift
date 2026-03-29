import XCTest
@testable import Residuum

final class AgentRegistryTests: XCTestCase {

    func testParsesEmptyFile() throws {
        let registry = try AgentRegistry.parse("")
        XCTAssertTrue(registry.agents.isEmpty)
    }

    func testParsesSingleAgent() throws {
        let toml = """
        [[agents]]
        name = "Aria"
        port = 7701
        """
        let registry = try AgentRegistry.parse(toml)
        XCTAssertEqual(registry.agents.count, 1)
        XCTAssertEqual(registry.agents[0].name, "Aria")
        XCTAssertEqual(registry.agents[0].port, 7701)
    }

    func testParsesMultipleAgents() throws {
        let toml = """
        [[agents]]
        name = "Aria"
        port = 7701

        [[agents]]
        name = "Sentinel"
        port = 7702
        """
        let registry = try AgentRegistry.parse(toml)
        XCTAssertEqual(registry.agents.count, 2)
        XCTAssertEqual(registry.agents[1].name, "Sentinel")
        XCTAssertEqual(registry.agents[1].port, 7702)
    }

    func testIgnoresUnknownKeys() throws {
        let toml = """
        [[agents]]
        name = "Aria"
        port = 7701
        description = "my agent"
        """
        let registry = try AgentRegistry.parse(toml)
        XCTAssertEqual(registry.agents.count, 1)
    }

    func testMissingPortIsSkipped() throws {
        let toml = """
        [[agents]]
        name = "Broken"
        """
        let registry = try AgentRegistry.parse(toml)
        XCTAssertTrue(registry.agents.isEmpty, "agents without a port should be skipped")
    }

    func testRegistryURLContainsExpectedPath() {
        let url = AgentRegistry.registryURL()
        XCTAssertNotNil(url)
        XCTAssertTrue(url!.path.contains(".residuum/agent_registry/registry.toml"))
    }
}
