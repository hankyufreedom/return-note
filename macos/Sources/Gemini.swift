import Foundation
import Security

enum GeminiKey {
    private static let identity: [String: Any] = [kSecClass as String: kSecClassGenericPassword,
        kSecAttrService as String: "io.github.wsxqaza12.return-note.gemini", kSecAttrAccount as String: "Gemini API key"]

    static func read() throws -> String? {
        var query = identity
        query[kSecReturnData as String] = true
        var item: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &item)
        if status == errSecItemNotFound { return nil }
        guard status == errSecSuccess, let data = item as? Data, let key = String(data: data, encoding: .utf8) else {
            throw NoteError("無法讀取鑰匙圈的 Gemini API key。")
        }
        return key
    }

    static func save(_ key: String) throws {
        guard !key.isEmpty, key.utf8.count <= 512,
              key.utf8.allSatisfy({ $0 >= 33 && $0 <= 126 }) else { throw NoteError("Gemini API key 格式不正確。") }
        let values = [kSecValueData as String: Data(key.utf8)]
        var status = SecItemUpdate(identity as CFDictionary, values as CFDictionary)
        if status == errSecItemNotFound { status = SecItemAdd(identity.merging(values) { _, new in new } as CFDictionary, nil) }
        guard status == errSecSuccess else { throw NoteError("無法儲存 Gemini API key 到鑰匙圈。") }
    }

    static func remove() throws {
        let status = SecItemDelete(identity as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else { throw NoteError("無法從鑰匙圈移除 Gemini API key。") }
    }
}

struct GeminiJobEnvelope: Decodable { let job: GeminiJob }
struct GeminiJob: Decodable {
    let model: String
    let tags: [[String]]
    let revision: UInt64
    let body: JSONValue
}

// Decodes the request constructed by the shared Rust core without interpreting its schema.
enum JSONValue: Codable {
    case null, bool(Bool), number(Double), string(String), array([JSONValue]), object([String: JSONValue])
    init(from decoder: Decoder) throws {
        let c = try decoder.singleValueContainer()
        if c.decodeNil() { self = .null }
        else if let value = try? c.decode(Bool.self) { self = .bool(value) }
        else if let value = try? c.decode(Double.self) { self = .number(value) }
        else if let value = try? c.decode(String.self) { self = .string(value) }
        else if let value = try? c.decode([JSONValue].self) { self = .array(value) }
        else { self = .object(try c.decode([String: JSONValue].self)) }
    }
    func encode(to encoder: Encoder) throws {
        var c = encoder.singleValueContainer()
        switch self {
        case .null: try c.encodeNil()
        case .bool(let value): try c.encode(value)
        case .number(let value): try c.encode(value)
        case .string(let value): try c.encode(value)
        case .array(let value): try c.encode(value)
        case .object(let value): try c.encode(value)
        }
    }
}

private final class NoRedirect: NSObject, URLSessionTaskDelegate {
    func urlSession(_ session: URLSession, task: URLSessionTask, willPerformHTTPRedirection response: HTTPURLResponse,
                    newRequest request: URLRequest, completionHandler: @escaping (URLRequest?) -> Void) {
        // Never forward a credential or note text to another host.
        completionHandler(nil)
    }
}

final class Gemini {
    private let session: URLSession = {
        let config = URLSessionConfiguration.ephemeral
        config.timeoutIntervalForRequest = 15
        config.timeoutIntervalForResource = 25
        config.httpShouldSetCookies = false
        config.urlCredentialStorage = nil
        return URLSession(configuration: config, delegate: NoRedirect(), delegateQueue: nil)
    }()
    deinit { session.invalidateAndCancel() }

    func send(job: GeminiJob, key: String, completion: @escaping (Result<(Int, Any), Error>) -> Void) throws -> URLSessionDataTask {
        guard job.model.range(of: "^[A-Za-z0-9.-]{1,100}$", options: .regularExpression) != nil,
              let endpoint = URL(string: "https://generativelanguage.googleapis.com/v1beta/models/\(job.model):generateContent") else {
            throw NoteError("Gemini 模型名稱格式不正確。")
        }
        var request = URLRequest(url: endpoint)
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.setValue(key, forHTTPHeaderField: "x-goog-api-key")
        request.httpBody = try JSONEncoder().encode(job.body)
        let task = session.dataTask(with: request) { data, response, error in
            guard error == nil, let http = response as? HTTPURLResponse, let data, data.count <= 2_000_000 else {
                completion(.failure(NoteError("Gemini 連線失敗或回應逾時；便條仍保留在本機。"))); return
            }
            let value = (try? JSONSerialization.jsonObject(with: data)) ?? NSNull()
            completion(.success((http.statusCode, value)))
        }
        task.resume()
        return task
    }
}
