# Yoinks: Implementation Plan

Yoinks is a native macOS drag-and-drop shelf written in Rust, inspired by
[Yoink for Mac](https://eternalstorms.at/yoink/mac/). It gives you a temporary
place to park files, text, images and links while you move between windows,
Spaces and full-screen apps.

## 1. Goals and non-goals

**Goals**
- Feel native: AppKit UI, real drag sessions, Finder-like move/copy semantics.
- Low overhead: idle CPU near 0%, small memory footprint, fast launch.
- Privacy first: everything stays local, sensitive clipboard content is skipped.
- Scriptable: CLI, URL scheme and App Intents (Shortcuts) support.

**Non-goals (for v1)**
- iOS/iPadOS companion app and Handoff to iOS (revisit after v1).
- Cross-platform support. The app is macOS-only by design.

## 2. Technology choices

| Concern | Choice | Notes |
| --- | --- | --- |
| Language | Rust (edition 2024) | |
| macOS bindings | `objc2`, `objc2-foundation`, `objc2-app-kit` | Type-safe, maintained, covers AppKit fully |
| Extra frameworks | `objc2-quick-look-ui`, `objc2-uniform-type-identifiers`, `objc2-quick-look-thumbnailing`, `block2` | Added as each phase needs them |
| Persistence | `serde` + `serde_json` (or `rusqlite` if history grows) | Stored under `~/Library/Application Support/Yoinks` |
| Global hotkeys | Carbon `RegisterEventHotKey` via FFI, or `global-hotkey` crate | No Accessibility permission needed for Carbon hotkeys |
| Logging | `tracing` + `tracing-oslog` | Shows up in Console.app |
| Bundling | `cargo-bundle` (metadata already in `Cargo.toml`), later a custom `xtask` | Produces `Yoinks.app`, then codesign + notarize |
| Minimum OS | macOS 13 Ventura | Unlocks modern APIs (SMAppService, App Intents) |

## 3. Architecture

```
src/
  main.rs            app bootstrap, NSApplication run loop
  app/
    delegate.rs      NSApplicationDelegate (launch, reopen, terminate)
    status_item.rs   menu bar icon and menu
  shelf/
    window.rs        NSPanel subclass: borderless, floating, non-activating
    view.rs          collection view of items, drag source + drop target
    item.rs          ShelfItem model (file ref, text, image, URL, promise)
    store.rs         ShelfStore: ordered items, persistence, observers
  drag/
    monitor.rs       global drag detection (NSEvent global monitor + drag pasteboard polling)
    session.rs       outgoing drag sessions, move/copy/link semantics
  clipboard/
    watcher.rs       NSPasteboard changeCount polling
    history.rs       bounded ring buffer + persistence
    filters.rs       password manager and ignored-app rules
  hotkey.rs          global shortcut: press, long press, double press
  preview.rs         thumbnails and QuickLook panel
  settings/
    model.rs         typed settings with serde defaults
    window.rs        preferences UI
  integrations/
    cli.rs           `yoinks add <path>` via a tiny IPC socket or URL scheme
    url_scheme.rs    yoinks://add?path=...
    services.rs      NSServices provider ("Add to Yoinks")
  util/
```

Key design points:
- **Main-thread discipline.** All AppKit work goes through `MainThreadMarker`.
  Background work (thumbnailing, file copies) runs on a worker pool and posts
  results back with `dispatch` main queue.
- **Security-scoped bookmarks** for file items, so items survive renames and
  moves, and so the app works when sandboxed (needed for the Mac App Store).
- **Model/view split.** `ShelfStore` is plain Rust and unit-testable; the
  AppKit views observe it.

## 4. Feature roadmap

### Phase 0: Scaffold (done)
- [x] `cargo init`, git repo, `objc2` dependencies.
- [x] Accessory (no Dock icon) app with a menu bar status item and Quit.

### Phase 1: The shelf window
- Borderless `NSPanel` with `NSWindowStyleMask::NonactivatingPanel`, floating
  level, `canJoinAllSpaces` + `fullScreenAuxiliary` collection behavior so it
  appears over full-screen apps and on every Space.
- Vibrancy background (`NSVisualEffectView`), rounded corners.
- Show/hide with slide animation from a chosen screen edge (left/right),
  respecting the active display and the notch/menu bar.
- Status item menu: Show Shelf, Clear Shelf, Preferences, Quit.

### Phase 2: Drop in (accepting items)
- Register the shelf as a drop target for file URLs, file promises
  (`NSFilePromiseReceiver`, e.g. Mail attachments, Photos), plain/rich text,
  images, URLs and colors.
- Non-file content (text snippets, images) is materialized into a cache folder
  so it can later be dragged out as a real file.
- `ShelfItem` kinds: `File(bookmark)`, `Text`, `RichText`, `Image`, `Url`,
  `Promise(pending)`.
- Multi-item drops create a **stack** (one tile, expandable), which is a
  convenient way to keep related items together.

### Phase 3: Drag out (Finder semantics)
- Shelf tiles act as `NSDraggingSource`; write file URLs and native types to
  the pasteboard.
- Default move/copy behavior mirrors Finder: same volume moves, different
  volume copies. Option (⌥) forces copy, Command (⌘) forces move.
- After a successful drag out, remove the item (configurable: "keep items
  after dragging out").
- Remember recently removed items for **recall** (see hotkey long press).

### Phase 4: Automatic appearance
- Detect a drag anywhere on the system: global `NSEvent` monitor for
  `leftMouseDragged` combined with checking `NSPasteboard(name: .drag)`
  `changeCount`. This needs no Accessibility permission.
- Appearance modes (from Yoink):
  1. Immediately at the screen edge when a drag starts.
  2. Only when the cursor approaches the screen edge.
  3. At the mouse cursor.
- Auto-hide when the drag ends and the shelf is empty.
- Per-app suppression list: do not auto-show when the frontmost app is in the
  list (the hotkey still works).

### Phase 5: Global keyboard shortcut
- Configurable hotkey (default ⌃⌥Y) with three gestures:
  - **Press:** toggle the shelf.
  - **Long press:** recall the items most recently dragged out.
  - **Double press:** save the current clipboard contents to the shelf.
- Shortcut recorder control in Preferences.

### Phase 6: Previews
- Tile thumbnails via `QLThumbnailGenerator`, cached on disk.
- Space bar / click opens the full `QLPreviewPanel` for the selected item.
- Hover tooltip with name, size, kind and original location.

### Phase 7: Clipboard history
- Poll `NSPasteboard.general.changeCount` (about every 0.5 s, coalesced with
  a dispatch timer to stay cheap).
- Bounded history (default 50 entries), persisted, searchable.
- Actions per entry: copy back to clipboard, send to shelf, delete, pin.
- Privacy:
  - Respect `org.nspasteboard.ConcealedType` and `TransientType` markers
    (used by 1Password, Bitwarden, KeePassXC and others).
  - Built-in ignore list of popular password managers, plus a user
    "Ignored Apps" list.
  - One-click "Pause clipboard history" from the menu bar.
- UI: popover from the status item. (Yoink uses a Notification Center widget;
  a WidgetKit extension cannot be built in pure Rust, so it is a stretch goal.)

### Phase 8: Preferences
- General: launch at login (`SMAppService.mainApp`), shelf edge, appearance
  mode, keep items after drag out, show in Dock toggle.
- Shortcut: hotkey recorder and gesture timing.
- Clipboard: enable, history size, ignored apps.
- Apps: per-app auto-show suppression.
- Storage: cache location, cache size, "clear cached files".

### Phase 9: Integrations
- **CLI:** `yoinks add <paths...>`, `yoinks list`, `yoinks clear`. The binary
  detects whether an instance is running and forwards commands over a Unix
  socket in the Application Support folder.
- **URL scheme:** `yoinks://add?path=...` and `yoinks://show`.
- **Services menu / Quick Action:** "Add to Yoinks" via `NSServices` in
  Info.plist, which also covers Automator workflows.
- **Share extension:** requires an app extension target; plan as a small Swift
  shim, or skip for v1.
- **Shortcuts (App Intents):** stretch goal, also needs a Swift shim.
- **Continuity Camera:** "Import from iPhone or iPad" in the shelf context
  menu via `NSMenuItem.importFromDeviceIdentifier` and the
  `NSServicesMenuRequestor` protocol.

### Phase 10: Packaging and distribution
- `Info.plist` with `LSUIElement = true`, URL scheme, services, usage strings.
- App icon (`.icns`) and template menu bar icon.
- Hardened runtime, Developer ID signing, notarization via `notarytool`.
- Optional App Sandbox build for the Mac App Store (needs bookmarks from
  Phase 2 and entitlement review).
- Auto-updates with Sparkle (via a small framework embed) for direct builds.
- GitHub Actions: `cargo fmt --check`, `cargo clippy -D warnings`,
  `cargo test`, universal binary (`aarch64` + `x86_64` with `lipo`).

## 5. Improvements over Yoink

| Idea | Description |
| --- | --- |
| Multiple named shelves | Switch between shelves (for example "Invoices", "Design assets"), each persisted separately |
| Pinned items | Pin items that should never be auto-removed after dragging out |
| Search and filter | Type to filter shelf and clipboard history by name, kind or content |
| Quick actions on items | Right-click: rename, compress to zip, convert image format, resize image, copy path, reveal in Finder, open with |
| Text snippet editing | Edit a text clipping inline before dragging it out |
| Auto-expiry | Optionally remove items older than N hours/days |
| OCR on images | Use the Vision framework to copy text out of an image or screenshot |
| Screenshot capture | Optionally watch the screenshot folder and add new screenshots automatically |
| Keyboard-only workflow | Arrow keys to select, Return to open, ⌘C to copy, Delete to remove, ⌘V to paste into the shelf |
| Paste to shelf | ⌘V while the shelf is focused adds the clipboard contents |
| Open source CLI + scripting | First-class CLI and JSON output for scripting (`yoinks list --json`) |
| Sensitive content redaction | Detect and skip likely secrets (API keys, tokens) in clipboard history |
| Low-resource mode | Disable thumbnails and polling on battery power |

## 6. Testing strategy

- **Unit tests** for pure Rust logic: `ShelfStore`, settings migration,
  clipboard filters, move/copy decision table, hotkey gesture state machine.
- **Integration tests** for the CLI and IPC socket.
- **Manual QA checklist** per release: drag from Finder, Safari, Mail (file
  promises), Photos, Notes; full-screen apps; multiple displays; Spaces;
  Stage Manager; external volumes; sandboxed build.
- Instruments pass for idle CPU and memory.

## 7. Milestones

| Milestone | Phases | Outcome |
| --- | --- | --- |
| M1: Usable shelf | 1, 2, 3 | Manually opened shelf with drop in and drag out |
| M2: Yoink parity core | 4, 5, 6 | Auto-appearing shelf, hotkey gestures, previews |
| M3: Clipboard + settings | 7, 8 | Clipboard history and a full preferences window |
| M4: Integrations | 9 | CLI, URL scheme, Services, Continuity Camera |
| M5: Ship | 10 | Signed, notarized `Yoinks.app` with CI |
| M6: Beyond Yoink | Section 5 | Selected improvements, prioritized after M5 |

## 8. Open questions

- Distribute through the Mac App Store (sandbox constraints) or direct only?
- Is a small Swift shim acceptable for Share extension, widgets and App
  Intents, or must the project stay 100% Rust?
- Should iCloud sync of shelves across Macs be in scope?
