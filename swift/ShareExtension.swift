// The Share extension: "Yoinks" in other apps' Share menus. It hands what
// is shared to the app through `yoinks://add` links (files by path, links
// and text as they are) and closes without showing anything.
//
// It runs sandboxed, so content that only exists as data (an image from
// Photos, say) is written into the extension's own temporary folder, which
// Yoinks (not sandboxed) can read.

import AppKit
import UniformTypeIdentifiers

final class ShareViewController: NSViewController {
    override func loadView() {
        view = NSView(frame: NSRect(x: 0, y: 0, width: 1, height: 1))
    }

    override func viewDidLoad() {
        super.viewDidLoad()
        let items = extensionContext?.inputItems as? [NSExtensionItem] ?? []
        let providers = items.flatMap { $0.attachments ?? [] }
        // Shared text often comes as the item's content text instead of an
        // attachment.
        let texts = items.compactMap { $0.attributedContentText?.string }
            .filter { !$0.isEmpty }
        Task { @MainActor in
            var shared = Shared()
            for provider in providers {
                await shared.add(provider)
            }
            if shared.texts.isEmpty {
                shared.texts = texts
            }
            shared.send()
            extensionContext?.completeRequest(returningItems: nil)
        }
    }
}

/// What was shared, gathered into as few requests as possible.
private struct Shared {
    var paths: [String] = []
    var links: [(url: String, title: String?)] = []
    var texts: [String] = []

    mutating func add(_ provider: NSItemProvider) async {
        if provider.hasItemConformingToTypeIdentifier(UTType.fileURL.identifier),
            let url = await loadURL(provider, UTType.fileURL)
        {
            paths.append(url.path)
        } else if provider.hasItemConformingToTypeIdentifier(UTType.url.identifier),
            let url = await loadURL(provider, UTType.url)
        {
            links.append((url.absoluteString, provider.suggestedName))
        } else if provider.hasItemConformingToTypeIdentifier(UTType.image.identifier),
            let path = await saveImage(provider)
        {
            paths.append(path)
        } else if provider.hasItemConformingToTypeIdentifier(UTType.plainText.identifier),
            let text = await loadText(provider)
        {
            texts.append(text)
        }
    }

    /// Opens one link for all files (one stack) and one per link or text.
    func send() {
        if !paths.isEmpty {
            open(paths.map { URLQueryItem(name: "path", value: $0) })
        }
        for link in links {
            var items = [URLQueryItem(name: "url", value: link.url)]
            if let title = link.title, !title.isEmpty {
                items.append(URLQueryItem(name: "title", value: title))
            }
            open(items)
        }
        for text in texts {
            open([URLQueryItem(name: "text", value: text)])
        }
    }

    private func open(_ items: [URLQueryItem]) {
        var components = URLComponents()
        components.scheme = "yoinks"
        components.host = "add"
        components.queryItems = items
        // `+` would read as a space in a query.
        components.percentEncodedQuery = components.percentEncodedQuery?
            .replacingOccurrences(of: "+", with: "%2B")
        if let url = components.url {
            NSWorkspace.shared.open(url)
        }
    }
}

private func loadURL(_ provider: NSItemProvider, _ type: UTType) async -> URL? {
    await withCheckedContinuation { continuation in
        provider.loadItem(forTypeIdentifier: type.identifier) { item, _ in
            switch item {
            case let url as URL: continuation.resume(returning: url)
            case let data as Data: continuation.resume(returning: URL(dataRepresentation: data, relativeTo: nil))
            case let string as String: continuation.resume(returning: URL(string: string))
            default: continuation.resume(returning: nil)
            }
        }
    }
}

private func loadText(_ provider: NSItemProvider) async -> String? {
    await withCheckedContinuation { continuation in
        provider.loadItem(forTypeIdentifier: UTType.plainText.identifier) { item, _ in
            switch item {
            case let string as String: continuation.resume(returning: string)
            case let text as NSAttributedString: continuation.resume(returning: text.string)
            case let data as Data: continuation.resume(returning: String(data: data, encoding: .utf8))
            default: continuation.resume(returning: nil)
            }
        }
    }
}

/// Writes a shared image into this extension's temporary folder.
private func saveImage(_ provider: NSItemProvider) async -> String? {
    let type = provider.registeredContentTypes.first { $0.conforms(to: .image) } ?? .png
    let name = (provider.suggestedName ?? "Image") + "." + (type.preferredFilenameExtension ?? "png")
    return await withCheckedContinuation { continuation in
        _ = provider.loadDataRepresentation(for: type) { data, _ in
            guard let data else {
                continuation.resume(returning: nil)
                return
            }
            let folder = FileManager.default.temporaryDirectory
                .appendingPathComponent(UUID().uuidString, isDirectory: true)
            let url = folder.appendingPathComponent(name)
            do {
                try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
                try data.write(to: url)
                continuation.resume(returning: url.path)
            } catch {
                continuation.resume(returning: nil)
            }
        }
    }
}
