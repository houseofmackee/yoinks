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
- Mac App Store distribution. v1 ships as a direct download only.
- Syncing shelves across Macs (kept as a design option, see section 8).

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
    geometry.rs      pure layout math for the panel
    view.rs          drop target content view and item tiles
    model.rs         ShelfModel: ties store, items folder and view together
    item.rs          Entry/Item model (file ref, text, image, URL, promise)
    store.rs         ShelfStore: ordered entries and persistence
    materialize.rs   writes dropped content into owned files
    pasteboard.rs    reads drag pasteboards
    bookmark.rs      file bookmarks
    promise.rs       file promise + content writer for dragging content out
    recent.rs        recall buffer of dragged-out entries
  drag/
    monitor.rs       shows/hides the shelf for system-wide drags
    tracker.rs       pure drag recognition state machine
  clipboard/
    watcher.rs       NSPasteboard changeCount polling
    history.rs       bounded ring buffer + persistence
    filters.rs       password manager and ignored-app rules
  hotkey.rs          global shortcut: press, long press, double press
  preview.rs         thumbnails and QuickLook panel
  settings.rs        typed settings with serde defaults (settings.json)
  settings/          (Phase 8) preferences UI
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
  moves. This also keeps a future sandboxed build possible.
- **Model/view split.** `ShelfStore` is plain Rust and unit-testable; the
  AppKit views observe it.

## 4. Feature roadmap

### Phase 0: Scaffold (done)
- [x] `cargo init`, git repo, `objc2` dependencies.
- [x] Accessory (no Dock icon) app with a menu bar status item and Quit.

### Phase 1: The shelf window (done)
- [x] Borderless `NSPanel` with `NSWindowStyleMask::NonactivatingPanel`, floating
  level, `canJoinAllSpaces` + `fullScreenAuxiliary` collection behavior so it
  appears over full-screen apps and on every Space.
- [x] Vibrancy background (`NSVisualEffectView`), rounded corners.
- [x] Show/hide with slide animation from a chosen screen edge (left/right),
  on the display under the cursor, within the visible frame (clear of the
  menu bar, notch and Dock). Fades instead of sliding when Reduce Motion is on.
- [x] Escape hides the shelf; relaunching the app shows it.
- [x] Status item menu: Show/Hide Shelf, Clear Shelf, Preferences, Quit.
  Clear Shelf and Preferences stay disabled until Phases 2 and 8.
- The edge is the `shelf_edge` setting (see Phase 4 for `settings.json`).

### Phase 2: Drop in (accepting items) (done)
- [x] The shelf is a drop target for file URLs, file promises
  (`NSFilePromiseReceiver`, e.g. Mail attachments, Photos), web links, images
  (PNG, JPEG, TIFF converted to PNG), rich text and plain text.
- [x] Non-file content is materialized into
  `~/Library/Application Support/Yoinks/Items/<item-id>/` (`.txt`, `.rtf`,
  `.png`, `.webloc`) so it can later be dragged out as a real file. Only
  these owned copies are ever deleted; the user's files never are.
- [x] `Item` kinds: `File`, `Text`, `RichText`, `Image`, `Url`, `Pending`.
  Entries and items have stable UUIDs and timestamps.
- [x] Multi-item drops create a **stack** (one tile, Finder multi-document
  icon). Right-click > Unstack splits it in place.
- [x] Shelf persisted to `shelf.json` (atomic writes, behind a
  `Persistence` trait). On launch, file bookmarks follow renamed or moved
  files and missing files are dropped. A corrupt file is set aside.
- [x] Tile context menu: Open, Reveal in Finder, Unstack, Remove.
  Double-click opens. Clear Shelf in the menu bar menu.
- `YOINKS_DATA_DIR` overrides the data folder (used for testing).
- Deferred: dropping colors (low value; revisit with Phase 6 previews).

### Phase 3: Drag out (Finder semantics) (done)
- [x] Tiles are `NSDraggingSource`s; a stack drags all its items.
- [x] Files and images go out as file URLs (images also carry their data).
  The source offers Copy, Move, Link and Generic and Finder applies its own
  rules: same volume moves, other volume copies, Option (⌥) copies, Command
  (⌘) moves, Option-Command makes an alias.
- [x] Text, rich text and links go out as a file promise plus their content:
  Finder writes a file, while text editors, text fields and browsers get the
  text or link (a file URL makes TextEdit insert an attachment instead).
- [x] After a successful drag out the entry leaves the shelf, unless the
  `keep_after_drag_out` setting is on; kept items follow their files if
  Finder moved them.
- [x] The last 10 dragged-out entries can be recalled (menu bar: Recall
  Last Dragged Item; hotkey long press in Phase 5). Recall follows moved
  files via bookmarks. A file Yoinks created that was moved out becomes the
  user's and is never deleted.
- [x] Dropping a tile back onto the shelf is refused.
- [x] Owned files no longer referenced (recall buffer at quit, crashes) are
  removed on launch.
- Not offered: dragging to the Trash (Delete operation), to avoid trashing
  original files by accident. Remove from the context menu instead.
- Known limit: recall cannot follow a file that Finder moved to another
  volume (that is a copy plus delete, so the bookmark points at nothing).

### Phase 4: Automatic appearance (done)
- [x] System-wide drag detection: a global `NSEvent` monitor for left mouse
  down/dragged, and a press only counts as a drag once the drag pasteboard's
  `changeCount` changes. Window moves, text selection and rubber-band
  selection never show the shelf. No Accessibility permission needed.
- [x] Drag end is detected by polling the mouse button (global monitors do
  not reliably see the mouse-up of another app's drag).
- [x] Appearance modes (`appear_on_drag`): `at_edge` (default), `near_edge`
  (within 40 pt of the shelf's screen edge), `at_cursor` (fades in beside
  the cursor), `off`.
- [x] Auto-hide when the drag ends, unless something was dropped on the
  shelf or the cursor is over it. A shelf opened by hand is left alone.
- [x] Per-app suppression: `ignored_apps` bundle ids (the frontmost app at
  drag start); the menu bar and shortcut still work.
- [x] Settings moved forward from Phase 8: typed `settings.json` in the data
  folder with serde defaults, written on first launch. Changes take effect
  on relaunch until the Preferences window exists.

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
- UI: popover from the status item.
- Widget: a WidgetKit extension (Swift add-on, see Phase 9) showing recent
  clipboard entries, like Yoink's Notification Center widget.

### Phase 8: Preferences
- The settings model and `settings.json` already exist (Phase 4); this
  phase adds the window and live updates.
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
- **Swift add-ons:** app extensions and App Intents cannot be built in pure
  Rust, so they live in a small Swift package under `swift/`. They talk to the
  Rust app through the URL scheme or the CLI socket, keeping the Swift side
  thin. The Rust app remains the single source of truth.
- **Share extension (Swift):** "Yoinks" in every Share menu.
- **Shortcuts (Swift App Intents):** "Add to Shelf", "Get Shelf Items",
  "Clear Shelf", "Show Shelf".
- **Continuity Camera:** "Import from iPhone or iPad" in the shelf context
  menu via `NSMenuItem.importFromDeviceIdentifier` and the
  `NSServicesMenuRequestor` protocol.

### Phase 10: Packaging and distribution
- `Info.plist` with `LSUIElement = true`, URL scheme, services, usage strings.
- App icon (`.icns`) and template menu bar icon.
- Hardened runtime, Developer ID signing, notarization via `notarytool`.
- Direct download only: a signed, notarized `.dmg` hosted on GitHub Releases.
- Auto-updates with Sparkle (via a small framework embed).
- Build step that compiles the Swift add-ons and embeds them in
  `Yoinks.app/Contents/PlugIns`.
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
  Stage Manager; external volumes.
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

## 8. Decisions and future options

**Decided**
- Distribution: direct download only for now. No Mac App Store build.
- Swift: small Swift add-ons are acceptable for the Share extension,
  widgets and Shortcuts (App Intents). Everything else stays in Rust.

**Future design option: sharing shelves across Macs**
- Not in scope yet. Keep the door open by giving every shelf item a stable
  UUID and a modification timestamp, and by keeping `ShelfStore` persistence
  behind a trait so a sync backend (iCloud via CloudKit, or a shared folder)
  can be added later without reworking the model.
