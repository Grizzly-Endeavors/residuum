import XCTest
@testable import Residuum

final class ProtocolTests: XCTestCase {
    private let encoder = JSONEncoder()
    private let decoder = JSONDecoder()

    // MARK: - ClientMessage encoding

    func testEncodeSendMessage() throws {
        let msg = ClientMessage.sendMessage(id: "abc", content: "hello", images: [])
        let data = try encoder.encode(msg)
        let json = try JSONSerialization.jsonObject(with: data) as! [String: Any]
        XCTAssertEqual(json["type"] as? String, "send_message")
        XCTAssertEqual(json["id"] as? String, "abc")
        XCTAssertEqual(json["content"] as? String, "hello")
        XCTAssertNil(json["images"], "empty images array should be omitted")
    }

    func testEncodeSendMessageWithImage() throws {
        let image = ImageData(mediaType: "image/png", data: "base64data")
        let msg = ClientMessage.sendMessage(id: "x", content: "look", images: [image])
        let data = try encoder.encode(msg)
        let json = try JSONSerialization.jsonObject(with: data) as! [String: Any]
        let images = json["images"] as? [[String: Any]]
        XCTAssertEqual(images?.count, 1)
        XCTAssertEqual(images?.first?["media_type"] as? String, "image/png")
        XCTAssertEqual(images?.first?["data"] as? String, "base64data")
    }

    func testEncodeSetVerbose() throws {
        let msg = ClientMessage.setVerbose(enabled: true)
        let data = try encoder.encode(msg)
        let json = try JSONSerialization.jsonObject(with: data) as! [String: Any]
        XCTAssertEqual(json["type"] as? String, "set_verbose")
        XCTAssertEqual(json["enabled"] as? Bool, true)
    }

    func testEncodePing() throws {
        let msg = ClientMessage.ping
        let data = try encoder.encode(msg)
        let json = try JSONSerialization.jsonObject(with: data) as! [String: Any]
        XCTAssertEqual(json["type"] as? String, "ping")
    }

    func testEncodeServerCommand() throws {
        let msg = ClientMessage.serverCommand(name: "observe", args: nil)
        let data = try encoder.encode(msg)
        let json = try JSONSerialization.jsonObject(with: data) as! [String: Any]
        XCTAssertEqual(json["type"] as? String, "server_command")
        XCTAssertEqual(json["name"] as? String, "observe")
        XCTAssertNil(json["args"])
    }

    // MARK: - ServerMessage decoding

    func testDecodeTurnStarted() throws {
        let json = #"{"type":"turn_started","reply_to":"corr-1"}"#.data(using: .utf8)!
        let msg = try decoder.decode(ServerMessage.self, from: json)
        guard case .turnStarted(let replyTo) = msg else {
            return XCTFail("expected turnStarted, got \(msg)")
        }
        XCTAssertEqual(replyTo, "corr-1")
    }

    func testDecodeResponse() throws {
        let json = #"{"type":"response","reply_to":"corr-1","content":"Hello there"}"#.data(using: .utf8)!
        let msg = try decoder.decode(ServerMessage.self, from: json)
        guard case .response(let replyTo, let content) = msg else {
            return XCTFail("expected response, got \(msg)")
        }
        XCTAssertEqual(replyTo, "corr-1")
        XCTAssertEqual(content, "Hello there")
    }

    func testDecodeToolCall() throws {
        let json = #"{"type":"tool_call","id":"tc1","name":"search","arguments":{"q":"test"}}"#.data(using: .utf8)!
        let msg = try decoder.decode(ServerMessage.self, from: json)
        guard case .toolCall(let id, let name, _) = msg else {
            return XCTFail("expected toolCall, got \(msg)")
        }
        XCTAssertEqual(id, "tc1")
        XCTAssertEqual(name, "search")
    }

    func testDecodeToolResult() throws {
        let json = #"{"type":"tool_result","tool_call_id":"tc1","name":"search","output":"found it","is_error":false}"#.data(using: .utf8)!
        let msg = try decoder.decode(ServerMessage.self, from: json)
        guard case .toolResult(let tcId, let name, let output, let isError) = msg else {
            return XCTFail("expected toolResult, got \(msg)")
        }
        XCTAssertEqual(tcId, "tc1")
        XCTAssertEqual(name, "search")
        XCTAssertEqual(output, "found it")
        XCTAssertFalse(isError)
    }

    func testDecodeError() throws {
        let json = #"{"type":"error","reply_to":"corr-1","message":"something went wrong"}"#.data(using: .utf8)!
        let msg = try decoder.decode(ServerMessage.self, from: json)
        guard case .error(let replyTo, let message) = msg else {
            return XCTFail("expected error, got \(msg)")
        }
        XCTAssertEqual(replyTo, "corr-1")
        XCTAssertEqual(message, "something went wrong")
    }

    func testDecodeErrorWithNilReplyTo() throws {
        let json = #"{"type":"error","message":"something went wrong"}"#.data(using: .utf8)!
        let msg = try decoder.decode(ServerMessage.self, from: json)
        guard case .error(let replyTo, _) = msg else {
            return XCTFail("expected error, got \(msg)")
        }
        XCTAssertNil(replyTo)
    }

    func testDecodeUnknownTypeDoesNotThrow() throws {
        let json = #"{"type":"future_message","data":"whatever"}"#.data(using: .utf8)!
        let msg = try decoder.decode(ServerMessage.self, from: json)
        guard case .unknown = msg else {
            return XCTFail("expected unknown, got \(msg)")
        }
    }
}
