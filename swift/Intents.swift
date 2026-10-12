// Shortcuts actions (App Intents) for Yoinks: add files, text and links,
// get what is on the shelf, show, hide, clear, recall and switch shelves.
// Each action sends a request in the command line's JSON format to Rust
// (`yoinks_request`), so it behaves exactly like `yoinks add`, `yoinks list`
// and the rest.

import AppIntents
import Foundation

@_silgen_name("yoinks_request")
private func yoinksRequest(_ json: UnsafePointer<CChar>) -> UnsafeMutablePointer<CChar>?

@_silgen_name("yoinks_free_string")
private func yoinksFreeString(_ string: UnsafeMutablePointer<CChar>)

struct YoinksError: Error, CustomLocalizedStringResourceConvertible {
    let message: String
    var localizedStringResource: LocalizedStringResource { "\(message)" }
}

private struct Response: Decodable {
    struct Entry: Decodable {
        struct Item: Decodable {
            let title: String
            let kind: String
            let path: String
            let url: String?
        }
        let title: String
        let items: [Item]
    }
    let ok: Bool
    let error: String?
    let entries: [Entry]
    let shelf: String?
    let shelves: [String]?
}

/// Sends `request` (with `"cmd"`) to the app and returns its answer.
@MainActor
@discardableResult
private func send(_ request: [String: Any]) throws -> Response {
    let data = try JSONSerialization.data(withJSONObject: request)
    guard let json = String(data: data, encoding: .utf8),
        let reply = json.withCString({ yoinksRequest($0) })
    else {
        throw YoinksError(message: "Yoinks did not answer")
    }
    let text = String(cString: reply)
    yoinksFreeString(reply)
    let response = try JSONDecoder().decode(Response.self, from: Data(text.utf8))
    if !response.ok {
        throw YoinksError(message: response.error ?? "Yoinks could not do that")
    }
    return response
}

struct AddFilesIntent: AppIntent {
    static var title: LocalizedStringResource { "Add Files to Shelf" }
    static var description: IntentDescription {
        "Put files on the Yoinks shelf. Several files become a stack."
    }

    @Parameter(title: "Files")
    var files: [IntentFile]

    @MainActor
    func perform() async throws -> some IntentResult {
        // Files from other apps may only exist as data: write those out.
        let folder = FileManager.default.temporaryDirectory
            .appendingPathComponent("Yoinks Shortcuts", isDirectory: true)
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        var paths: [String] = []
        for file in files {
            if let url = file.fileURL, FileManager.default.fileExists(atPath: url.path) {
                paths.append(url.path)
            } else {
                try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
                let url = folder.appendingPathComponent(file.filename.isEmpty ? "File" : file.filename)
                try file.data.write(to: url)
                paths.append(url.path)
            }
        }
        try send(["cmd": "add", "paths": paths])
        return .result()
    }
}

struct AddTextIntent: AppIntent {
    static var title: LocalizedStringResource { "Add Text to Shelf" }
    static var description: IntentDescription { "Put a text clipping on the Yoinks shelf." }

    @Parameter(title: "Text")
    var text: String

    @MainActor
    func perform() async throws -> some IntentResult {
        try send(["cmd": "add", "text": text])
        return .result()
    }
}

struct AddLinkIntent: AppIntent {
    static var title: LocalizedStringResource { "Add Link to Shelf" }
    static var description: IntentDescription { "Put a link on the Yoinks shelf." }

    @Parameter(title: "Link")
    var link: URL

    @Parameter(title: "Title")
    var linkTitle: String?

    @MainActor
    func perform() async throws -> some IntentResult {
        var request: [String: Any] = ["cmd": "add", "url": link.absoluteString]
        if let linkTitle, !linkTitle.isEmpty {
            request["title"] = linkTitle
        }
        try send(request)
        return .result()
    }
}

struct GetShelfItemsIntent: AppIntent {
    static var title: LocalizedStringResource { "Get Shelf Items" }
    static var description: IntentDescription {
        "Get the files on the Yoinks shelf (text and links as the files Yoinks keeps for them), newest first."
    }

    @MainActor
    func perform() async throws -> some IntentResult & ReturnsValue<[IntentFile]> {
        let response = try send(["cmd": "list"])
        let files = response.entries
            .flatMap(\.items)
            .filter { $0.kind != "pending" }
            .map { IntentFile(fileURL: URL(fileURLWithPath: $0.path)) }
        return .result(value: files)
    }
}

struct ShowShelfIntent: AppIntent {
    static var title: LocalizedStringResource { "Show Shelf" }
    static var description: IntentDescription { "Show the Yoinks shelf." }

    @MainActor
    func perform() async throws -> some IntentResult {
        try send(["cmd": "show"])
        return .result()
    }
}

struct HideShelfIntent: AppIntent {
    static var title: LocalizedStringResource { "Hide Shelf" }
    static var description: IntentDescription { "Hide the Yoinks shelf." }

    @MainActor
    func perform() async throws -> some IntentResult {
        try send(["cmd": "hide"])
        return .result()
    }
}

struct ClearShelfIntent: AppIntent {
    static var title: LocalizedStringResource { "Clear Shelf" }
    static var description: IntentDescription {
        "Remove everything from the Yoinks shelf except pinned items."
    }

    @MainActor
    func perform() async throws -> some IntentResult {
        try send(["cmd": "clear"])
        return .result()
    }
}

struct RecallIntent: AppIntent {
    static var title: LocalizedStringResource { "Bring Back Last Item" }
    static var description: IntentDescription {
        "Put the item you last dragged out of the shelf back on it."
    }

    @MainActor
    func perform() async throws -> some IntentResult {
        try send(["cmd": "recall"])
        return .result()
    }
}

/// The shelves' names, for picking one in Shortcuts.
struct ShelfNames: DynamicOptionsProvider {
    @MainActor
    func results() async throws -> [String] {
        try send(["cmd": "shelf"]).shelves ?? []
    }
}

struct SwitchShelfIntent: AppIntent {
    static var title: LocalizedStringResource { "Switch Shelf" }
    static var description: IntentDescription { "Show one of your named shelves." }

    @Parameter(title: "Shelf", optionsProvider: ShelfNames())
    var name: String

    @MainActor
    func perform() async throws -> some IntentResult {
        try send(["cmd": "shelf", "name": name])
        return .result()
    }
}

struct YoinksShortcuts: AppShortcutsProvider {
    static var appShortcuts: [AppShortcut] {
        AppShortcut(
            intent: ShowShelfIntent(),
            phrases: ["Show the \(.applicationName) shelf", "Show \(.applicationName)"],
            shortTitle: "Show Shelf",
            systemImageName: "tray.and.arrow.down"
        )
        AppShortcut(
            intent: RecallIntent(),
            phrases: ["Bring back the last item in \(.applicationName)"],
            shortTitle: "Bring Back Last Item",
            systemImageName: "arrow.uturn.backward"
        )
        AppShortcut(
            intent: ClearShelfIntent(),
            phrases: ["Clear the \(.applicationName) shelf"],
            shortTitle: "Clear Shelf",
            systemImageName: "trash"
        )
    }
}
