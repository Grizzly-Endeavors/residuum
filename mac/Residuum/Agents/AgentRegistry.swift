import Foundation

/// A single named agent entry from the registry.
struct AgentEntry: Equatable {
    let name: String
    let port: UInt16
}

/// Reads the Residuum agent registry from
/// `~/.residuum/agent_registry/registry.toml`.
///
/// The file format is a TOML array of tables:
/// ```toml
/// [[agents]]
/// name = "Aria"
/// port = 7701
/// ```
struct AgentRegistry {
    let agents: [AgentEntry]

    /// Load the registry from the standard location.
    /// Returns an empty registry if the file doesn't exist.
    static func load() -> AgentRegistry {
        guard let url = registryURL(),
              let contents = try? String(contentsOf: url, encoding: .utf8) else {
            return AgentRegistry(agents: [])
        }
        return (try? parse(contents)) ?? AgentRegistry(agents: [])
    }

    /// Parse a TOML string into an `AgentRegistry`.
    /// Only handles the specific format used by Residuum's registry.
    static func parse(_ toml: String) throws -> AgentRegistry {
        var agents: [AgentEntry] = []
        var currentName: String?
        var currentPort: UInt16?

        func flushCurrent() {
            if let name = currentName, let port = currentPort {
                agents.append(AgentEntry(name: name, port: port))
            }
            currentName = nil
            currentPort = nil
        }

        for line in toml.components(separatedBy: .newlines) {
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if trimmed == "[[agents]]" {
                flushCurrent()
            } else if trimmed.hasPrefix("name = ") {
                currentName = trimmed
                    .dropFirst("name = ".count)
                    .trimmingCharacters(in: CharacterSet(charactersIn: "\""))
            } else if trimmed.hasPrefix("port = ") {
                let raw = String(trimmed.dropFirst("port = ".count))
                    .trimmingCharacters(in: .whitespaces)
                currentPort = UInt16(raw)
            }
        }
        flushCurrent()

        return AgentRegistry(agents: agents)
    }

    /// URL of the registry file: `~/.residuum/agent_registry/registry.toml`.
    static func registryURL() -> URL? {
        guard let home = ProcessInfo.processInfo.environment["HOME"] else { return nil }
        return URL(fileURLWithPath: home)
            .appendingPathComponent(".residuum/agent_registry/registry.toml")
    }
}
