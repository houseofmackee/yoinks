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
| Extra frameworks | `objc2-quick-look-ui`, `objc2-quick-look-thumbnailing`, `objc2-uniform-type-identifiers`, `objc2-service-management`, `block2`, `dispatch2` | Added as each phase needed them |
| Persistence | `serde` + `serde_json` | Stored under `~/Library/Application Support/Yoinks`; `rusqlite` only if history ever outgrows JSON |
| Global hotkeys | Carbon `RegisterEventHotKey` via FFI | No Accessibility or Input Monitoring permission needed |
| Logging | `eprintln!` to stderr | `tracing` + `tracing-oslog` (Console.app) remains an option |
| Bundling | `scripts/bundle.sh` + `macos/Info.plist`, `scripts/make-dmg.sh`, `scripts/dist.sh` | Releases are built, signed with a Developer ID, notarized and published by `.github/workflows/release.yml`; Sparkle updates (`scripts/sparkle.sh`, `src/updates.rs`) |
| Minimum OS | macOS 13 Ventura | Unlocks modern APIs (SMAppService, App Intents) |

## 3. Architecture

```
src/
  main.rs            app bootstrap, NSApplication run loop
  app/
    delegate.rs      NSApplicationDelegate (launch, reopen, terminate)
    status_item.rs   menu bar icon and menu
    status_drop.rs   drops onto the menu bar icon
  shelf/
    window.rs        NSPanel subclass: borderless, floating, non-activating
    geometry.rs      pure layout math for the panel
    view.rs          content view: drop target, selection, keyboard, Quick Look
    tile.rs          one entry: thumbnail, title, badge; drag source
    thumbnail.rs     Quick Look thumbnail cache
    describe.rs      tooltip text
    model.rs         ShelfModel: ties store, items folder and view together
    item.rs          Entry/Item model (file ref, text, image, URL, promise)
    store.rs         ShelfStore: ordered entries and persistence
    materialize.rs   writes dropped content into owned files
    pasteboard.rs    reads drag pasteboards
    bookmark.rs      file bookmarks
    promise.rs       file promise + content writer for dragging content out
    recent.rs        recall buffer of dragged-out entries
    actions.rs       quick actions: zip, image convert/resize, rename rules
    prompt.rs        non-activating Rename prompt
    ocr.rs           Vision text recognition for Copy Text
    search.rs        matching entries against a shelf search
    shelves.rs       named shelves, color tags, shelves.json
  drag/
    monitor.rs       shows/hides the shelf for system-wide drags
    tracker.rs       pure drag recognition state machine
    shake.rs         pure shake-to-show detection
  clipboard/
    watcher.rs       NSPasteboard changeCount polling, recording, copy back
    history.rs       pure model: dedupe, pins, limit, search, persistence
    filters.rs       concealed markers, password managers, ignored apps
    secrets.rs       detects likely passwords, API keys and tokens
    panel.rs         non-activating history panel (search + table)
  hotkey/
    shortcut.rs      parse "control+option+Y" into Carbon key code + modifiers
    gesture.rs       pure press / long press / double press state machine
    carbon.rs        RegisterEventHotKey registration and dispatch
  folders.rs         watches the screenshot folder and watched folders
  power.rs           energy saving on battery or in Low Power Mode
  notch.rs           the notch as a drop target
  updates.rs         Sparkle, loaded at runtime from release bundles
  settings.rs        typed settings with serde defaults (settings.json)
  preferences/
    window.rs        tabs (General, Shelf, Adding, Shortcut, Clipboard, Storage)
    recorder.rs      shortcut recorder button
    app_list.rs      editable app list (+ from running apps)
    form.rs          layout helpers
    storage.rs       disk usage
  cli.rs             `yoinks add|list|show|hide|recall|clear`
  ipc/
    protocol.rs      JSON request/response types
    server.rs        Unix socket in the data folder; main-thread handler
    client.rs        command line side of the socket
    url.rs           yoinks:// link parsing
  app/commands.rs    answers requests from the socket and links
macos/Info.plist     bundle template (URL scheme, Services)
scripts/bundle.sh    builds, signs (ad hoc) and registers Yoinks.app
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
  folder with serde defaults, written on first launch. (Since Phase 8,
  changes made in Preferences apply immediately.)

### Phase 5: Global keyboard shortcut (done)
- [x] `hotkey` setting, default `control+option+Y` (`null` turns it off),
  registered with Carbon `RegisterEventHotKey`: no Accessibility or Input
  Monitoring permission needed.
- [x] Gestures from a pure, tested state machine:
  - **Press:** toggle the shelf (reported once the 250 ms double-press
    window has passed).
  - **Long press** (500 ms): recall the most recently dragged-out entry.
  - **Double press:** add the clipboard contents to the shelf.
  - Long and double press beep when there is nothing to recall or nothing
    usable on the clipboard.
- [x] Menu bar: "Add Clipboard to Shelf" item; the shortcut is shown next
  to Show/Hide Shelf.
- Shortcuts are written as text (`control+option+Y`, `cmd+shift+K`,
  `F5`): letters, digits, ANSI symbols, Space and F1 to F12; at least one
  modifier unless it is a function key. Invalid values are reported and
  ignored.
- Known limit: macOS does not report a clash with another app's shortcut;
  both apps receive it. The Phase 8 recorder warns about well-known macOS
  shortcuts.

### Phase 6: Previews (done)
- [x] Tile thumbnails from `QLThumbnailGenerator`, written straight to
  `Thumbnails/<item-id>.png` (2x the 56 pt icon). Regenerated when the file
  changes, requested at most once per file version per session, removed
  with their items and cleaned up on launch. Folders and links keep their
  Finder icons; stacks show the first item's thumbnail with a count badge.
- [x] Quick Look: Space (or the tile menu) toggles `QLPreviewPanel` for the
  selected entry; stacks preview all their items. Up/Down move through the
  shelf while previewing; the preview zooms from the tile's icon.
- [x] Selection and keyboard: click to select (accent highlight), Up/Down,
  Return opens, Delete removes (selection moves to the neighbor), Escape
  hides. Click the background to deselect.
- [x] Tooltips: name, kind, size, location of the user's file, the link
  URL, or a text excerpt. Like all AppKit tooltips they only appear while
  the shelf has keyboard focus (after clicking it or opening it with the
  shortcut or menu).
- [x] Fix: automatic appearance during a drag no longer takes keyboard
  focus, so typing keeps going to the app the user is working in. Opening
  the shelf on purpose still focuses it.

### Phase 7: Clipboard history (done)
- [x] Polls the general pasteboard's `changeCount` every 0.5 s. What is on
  the clipboard at launch, and anything copied while paused, is not
  recorded.
- [x] Records text, rich text, links, images (PNG/JPEG, up to 20 MB) and
  files (references); text over 1 MB is skipped. Large payloads live in
  `Clips/`, the rest in `clipboard.json`. Same content copied again moves
  to the top instead of duplicating.
- [x] Bounded history (`clipboard_history_size`, default 50 unpinned);
  pinned clips are never evicted or cleared.
- [x] Privacy: skips `org.nspasteboard.ConcealedType`, `TransientType`,
  `AutoGeneratedType` and similar markers, copies made in popular password
  managers (1Password, Bitwarden, KeePassXC, Keychain Access, Passwords,
  and others), and `clipboard_ignored_apps`. "Pause Clipboard History" in
  the menu bar (not persisted). `clipboard_history: false` turns it off.
- [x] History panel under the menu bar icon: search as you type, Up/Down,
  Return or double-click copies and closes; right-click to Copy, Add to
  Shelf, Pin/Unpin or Delete; "Clear Unpinned". Each clip shows the
  source app and age.
- [x] The panel is non-activating (like Spotlight): it takes typing without
  bringing Yoinks to the front, so after copying, Command-V goes straight
  to the app the user was in. It closes on Escape, a click elsewhere, or
  an app switch. (An NSPopover was tried first, but macOS 14+ refuses to
  activate a background menu bar app, so typing went elsewhere.)
- Not done: a WidgetKit extension showing recent clips, like Yoink's
  Notification Center widget. Part of the deferred Swift add-ons
  (Phase 9b).

### Phase 8: Preferences (done)
- [x] Preferences window (menu bar > Preferences…), a non-activating panel
  so it works without bringing Yoinks to the front. Every change applies
  immediately and is saved to `settings.json`.
- [x] General: open at login (`SMAppService.mainApp`, with a note when
  approval is needed), shelf edge, when to show the shelf during drags,
  keep items after dragging out, apps whose drags never show the shelf
  (picked from running apps).
- [x] Shortcut: recorder (click, press keys; Delete clears, Escape
  cancels), "Use ⌃⌥Y" reset, a warning for well-known macOS shortcuts
  (Spotlight, screenshots, Copy/Paste, ...) and for registration failures.
  The global shortcut is suspended while recording.
- [x] Clipboard: on/off, history size, apps never recorded, clear unpinned.
- [x] Storage: data folder with Show in Finder, space used by shelf copies,
  thumbnails and clipboard payloads, Clear Thumbnails.
- [x] Live updates: shelf edge, drag monitor, keep mode, clipboard watcher
  and shortcut (re-registered, menu rebuilt) without relaunching.
- Deferred: a "show in Dock" toggle (Yoinks has no app menu to show) and
  adjustable gesture timing.

### Phase 9a: Integrations in Rust (done)
- [x] `scripts/bundle.sh [debug|release]` builds `Yoinks.app` from
  `macos/Info.plist` (LSUIElement, `yoinks://` scheme, NSServices), signs it
  ad hoc and registers it with Launch Services. (Moved forward from
  Phase 10, which adds icons, Developer ID signing and notarization.)
- [x] Command line: the app binary doubles as `yoinks` (`add <files>`,
  `add --text`, `add --url [--title]`, `list [--json]`, `show`, `hide`,
  `recall`, `clear`, `help`). It talks to the running app over a Unix
  socket in the data folder (mode 0600) and starts the app in the
  background if needed. Exit codes: 0 ok, 1 failed, 2 bad usage.
- [x] One app per data folder: a second copy quits.
- [x] `yoinks://add?path=...&path=...`, `?text=`, `?url=&title=`,
  `yoinks://show`, `hide`, `recall`. Links can come from any web page, so
  nothing destructive (no `clear`).
- [x] Files opened with Yoinks (`open -a Yoinks file`, dropping on the app
  icon) go on the shelf as one entry.
- [x] "Add to Yoinks" service for files, text, links and images (Services
  menu, Finder Quick Actions).
- [x] Shelf background menu: Add Clipboard to Shelf, and "Import from
  iPhone or iPad" (Continuity Camera). The shelf is the receiver of the
  photo or scan. Not verified with a device (none was available).

### Phase 9b: Swift add-ons (Share extension and Shortcuts done; widget to do)
- Share extension ("Yoinks" in Share menus), Shortcuts actions (App
  Intents: Add to Shelf, Get Shelf Items, Clear Shelf, Show Shelf) and a
  widget showing recent clips, in a small Xcode project under `swift/`
  embedded into `Yoinks.app/Contents/PlugIns` by the bundle script.
- They talk to the Rust app through the `yoinks://` scheme (share, intents)
  or the socket; the widget reads a snapshot the app writes.
- Constraints: app extensions must be sandboxed and code signed; App
  Intents metadata needs `xcodebuild`; a widget needs an App Group shared
  with the app, which requires a Developer ID team.
- Decision (2026-10-10): skipped for now. Revisit once a signing team is
  available; without one, only the Share extension and Shortcuts actions
  could be tried, ad hoc and on one Mac.
- Update (2026-10-12): the Developer ID team (ZD5VPNGP3T) is now set up,
  so this is unblocked. An App Group for the widget still needs to be
  registered with that team.
- [x] Shortcuts actions (2026-10-12), built like CleanShotZ's without an
  Xcode project: `build.rs` compiles `swift/Intents.swift` into a static
  library linked into the app, and `bundle.sh` runs Xcode's
  `appintentsmetadataprocessor` for `Metadata.appintents`. Add Files, Add
  Text, Add Link, Get Shelf Items, Show, Hide, Clear, Bring Back Last
  Item and Switch Shelf (with the shelves offered as options). They send
  the command line's JSON requests to `yoinks_request` (src/app/
  intents.rs). Checked in the metadata and by a bridge test; trying them
  in Shortcuts needs the installed app.
- [x] Share extension (2026-10-12): `swift/ShareExtension.swift` built
  by `bundle.sh` into `Contents/PlugIns/YoinksShare.appex`, sandboxed,
  signed before the app and notarized with it. It hands files, links and
  text to the app through `yoinks://add` links; images that only exist
  as data are written into its container, and Yoinks takes them over.
  Tested by registering a dev build and sharing files, image data, text
  and a link through the share services.
- To do: the widget (recent shelf items or clips). Needs an App Group
  with the team and can only be tried from an installed, signed build.

### Phase 10: Packaging and distribution (done)
- [x] Apple Silicon only (decision 2026-10-10): no universal binary.
  `LSArchitecturePriority` is arm64.
- [x] App icon drawn by `scripts/make-icon.swift` into `macos/AppIcon.icns`
  (tray-and-arrow on a blue gradient, macOS icon grid); template SF Symbol
  in the menu bar.
- [x] Release profile: thin LTO, one codegen unit, stripped (about 1.2 MB).
- [x] `scripts/make-dmg.sh`: release bundle plus Applications shortcut in a
  compressed APFS disk image, `target/release/Yoinks-<version>.dmg`.
- [x] GitHub Actions (`.github/workflows/ci.yml`, Apple Silicon runner):
  `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`. Running on
  github.com/houseofmackee/yoinks since 2026-10-10.
- [x] Later (2026-10-11), aligned with CleanShotZ: CI runs on master and pull
  requests (clippy also in release, a release bundle build, `cargo deny`),
  and `.github/workflows/release.yml` builds, signs with the self-signed
  "Yoinks Release" certificate (hardened runtime) and publishes the DMG,
  zip and `SHA256SUMS.txt` when a `v*` tag is pushed; hyphenated tags
  become pre-releases. First used for v0.1.3-beta.1. The certificate stays
  in the login keychain as the local fallback; its repository secrets were
  deleted once releases moved to the Developer ID.
- [x] README with install, usage, command line, settings and build steps.
- [x] Developer ID signing and notarization (2026-10-12), set up like
  CleanShotZ with the same team's certificate and App Store Connect key:
  `bundle.sh` prefers a Developer ID (secure timestamp, hardened runtime),
  and `dist.sh` notarizes the app with `notarytool`, staples it, then signs,
  notarizes and staples the DMG. The release workflow imports the
  certificate (plus Apple's G2 intermediate) and refuses to publish a DMG
  that fails `stapler validate`. No more "Open Anyway" step. Stapling and
  validating are retried up to five times, because the CI runner's
  connection to Apple's ticket server dropped twice on the first v0.3.0
  run.
- [x] `yoinks shelf [<name>]` and a `shelf` request switch or list
  shelves; responses carry the current shelf and all shelf names.
- [x] README screenshots (2026-10-12) in `docs/screenshots`, staged
  with demo data in a scratch data folder in front of a plain backdrop
  window (the shelf and menus are translucent).
- [x] `CHANGELOG.md` (2026-10-12): each version's section is rendered
  by `scripts/release-notes.py` into the appcast (shown in Sparkle's
  update window) and put at the top of the GitHub release notes; the
  workflow warns if a tag has no section.
- [x] v0.3.0 (2026-10-12): the first full release that is Developer ID
  signed, notarized and carries Sparkle, with its `appcast.xml` as the
  update feed. Installed in /Applications from the release DMG.
- [x] Homebrew cask (2026-10-12): `Casks/yoinks.rb` in
  `houseofmackee/homebrew-tap` (app, `yoinks` binary, `auto_updates`,
  zap of the data folder and preferences). The release workflow updates
  its version and checksum for full releases, pushing with the write
  deploy key in `TAP_DEPLOY_KEY`, as CleanShotZ does.
- [x] v0.4.0 (2026-10-12): shelf search, energy saving and the Shelf
  preferences tab. The first real update over GitHub: the installed 0.3.0
  offered 0.4.0 with its changelog notes, then downloaded, verified,
  installed and relaunched it; the result is still notarized.
- [x] Shelf stuck on one desktop (2026-10-21): the shelf could end up tied
  to the desktop where it was once opened, despite `CanJoinAllSpaces`. Not
  reproducible from a fresh launch, so the shelf and the notch panel now
  repair it: on showing, and on every desktop switch
  (`NSWorkspaceActiveSpaceDidChangeNotification`), a visible window that
  is not on the active desktop is moved there (`MoveToActiveSpace`) and
  set back to all desktops (`ui::ensure_on_active_space`). Checked with a
  build whose shelf starts tied to one desktop: without the repair it
  stays behind, with it it follows.
- [x] v0.6.1 (2026-10-12): the shorter Preferences (new Adding tab) and
  windows that open on the current desktop. Installed through Sparkle;
  the cask followed.
- [x] Preferences fit again (2026-10-12): the Shelf tab had grown to
  about 860 points; notch, screenshots and watched folders moved to a new
  Adding tab (tallest tab now about 550). Preferences, Clipboard History
  and the prompts follow the active Space (`ui::follow_active_space`), so
  they no longer reopen on the desktop where they were first shown.
- [x] v0.6.0 (2026-10-12): reorder tiles by dragging, drop onto the menu
  bar icon, and the notch as an optional drop target, with README
  screenshots of both. The installed 0.5.0 updated to it through Sparkle;
  the cask followed automatically.
- [x] v0.5.0 (2026-10-12): named shelves with color tags, Share, shake to
  show, watched folders, the Share extension and Shortcuts actions. The
  release workflow updated the Homebrew cask by itself for the first time,
  and the installed 0.4.0 updated to it through Sparkle. CI had been
  failing on its older macos-14 runner (App Intents metadata processor,
  an incomplete cached Sparkle); it now runs on macos-15 like releases.
- [x] Automatic updates with Sparkle 2.10 (2026-10-12), as in CleanShotZ:
  `scripts/sparkle.sh` downloads it at a pinned checksum, `bundle.sh`
  embeds it in release builds (no XPC services) with the feed URL and
  public key, signing its executables inside out; `updates.rs` loads it at
  runtime, so debug builds simply have no Check for Updates…. `dist.sh`
  writes `appcast.xml` signed with the Yoinks EdDSA key (its own key, not
  CleanShotZ's; the private half is in the login keychain under account
  `io.github.yoinks` and in the `SPARKLE_PRIVATE_KEY` secret), and the
  release workflow attaches it. The feed is read from the latest full
  release, so pre-releases are never offered. `CFBundleVersion` drops the
  hyphen and dot (`0.2.0beta2`) because Sparkle treats `0.2.0-beta.1`,
  `0.2.0-beta.2` and `0.2.0` as equal. Tested end to end with a local
  feed: offered, downloaded, verified, replaced and relaunched.
- Swift add-ons (Phase 9b) would be built and embedded here; signing now
  exists (see section 9).

### Additions requested after Phase 10 (done)
- [x] Hide the menu bar icon: `show_menu_bar_icon` setting and a
  "Show Yoinks in the menu bar" checkbox in Preferences > General, applied
  live. With the icon hidden, the shelf is reached with the shortcut,
  automatic appearance, or by opening Yoinks again.
- [x] Settings from the shelf: a gear button at the bottom of the shelf
  opens the full app menu (Preferences…, Clipboard History…, Quit, ...)
  upwards, with nothing pre-selected. Clipboard History opened from there
  appears beside the shelf.

### Feedback from first use (done)
- [x] Revised: auto-hide only applies to an empty shelf. A shelf with items
  stays until the user hides it; emptying it (drag out, remove, clear)
  arms the auto-hide. Preferences label: "Hide the shelf once it's empty".
- [x] Hide after use (`hide_after_use`, on by default; Preferences >
  General): after a drop, a drag-out, or something added from the command
  line, Services or the shortcut, the shelf slides away once the pointer
  has been away for 1 s (3 s for additions the user didn't make on the
  shelf). Not while the pointer is on it, a mouse button is down, a menu is
  open or Quick Look shows one of its items. A shelf opened on purpose
  stays until it has been visited and left.
- [x] Movable shelf: drag it by its empty background (tiles, the gear and
  the scroll bar keep their own clicks). The position (`shelf_position`)
  is saved; the shelf then fades in and out there, on any of its
  appearance triggers. "Move Shelf Back to Edge" (menu bar and gear menu)
  and changing the edge in Preferences undo it.
- [x] Fix: Preferences starts every change from the settings in effect,
  instead of its own copy, so it cannot overwrite changes made elsewhere.

## 5. Improvements over Yoink

| Idea | Description | Status |
| --- | --- | --- |
| Multiple named shelves | Switch between shelves (for example "Invoices", "Design assets"), each persisted separately | Done: Shelves menu, ⌘1 to ⌘9, Finder-style color tags with an optional background tint; `shelves.json` plus one file per shelf |
| Pinned items | Pin shelf items so they stay after being dragged out | Done: pin badge, kept after drag out and Clear Shelf |
| Search and filter | Type to filter shelf and clipboard history by name, kind or content | Done: typing on the shelf or ⌘F filters it by name, link, kind and text contents |
| Quick actions on items | Right-click: rename, compress to zip, convert image format, resize image, copy path, open with | Done (rename is for items Yoinks saved; results are new items) |
| Text snippet editing | Edit a text clipping inline before dragging it out | To do |
| Auto-expiry | Optionally remove items older than N hours/days | Done: Preferences > General, 1 hour to 30 days; pinned items stay |
| OCR on images | Use the Vision framework to copy text out of an image or screenshot | Done: right-click > Copy Text |
| Screenshot capture | Optionally watch the screenshot folder and add new screenshots automatically | Done: Preferences > Shelf; uses the `kMDItemIsScreenCapture` marker, so other files in the folder are ignored. Watched folders (any folder, new files once settled) build on it |
| Keyboard-only workflow | Arrow keys to select, Return to open, ⌘C to copy, Delete to remove, ⌘V to paste into the shelf | Done |
| Paste to shelf | ⌘V while the shelf is focused adds the clipboard contents | Done |
| Open source CLI + scripting | First-class CLI and JSON output for scripting (`yoinks list --json`) | Done (Phase 9a) |
| Share menu | Send items with AirDrop, Mail, Messages and other share services | Done: right-click > Share (services listed directly; the share picker popover needs an active app) |
| Shake to show | Shake the pointer mid-drag to bring up the shelf (as in Dropover) | Done: a choice under "Show the shelf during drags" |
| Reorder tiles | Drag a tile up or down within the shelf | Done: within Yoinks a tile drag is a move; the shelf shows an insertion line and moves the entry |
| Drop on the menu bar icon | Drop items on the menu bar icon (or the notch area) to add them | Done: the icon's window is registered for drags (`src/app/status_drop.rs`), and on Macs with a notch a pill-sized panel over it draws only the notch until a drag arrives, so its clear pixels let clicks through (`src/notch.rs`, optional) |
| Sensitive content redaction | Detect and skip likely secrets (API keys, tokens) in clipboard history | Done: known key formats, private keys, credential lines and random tokens are skipped (Preferences > Clipboard) |
| Low-resource mode | Disable thumbnails and polling on battery power | Done: "Save energy on battery" (Preferences > General) slows clipboard and screenshot polling and skips new thumbnails on battery or in Low Power Mode |

## 6. Testing strategy

- **Unit tests** for pure Rust logic: `ShelfStore`, settings migration,
  clipboard filters, move/copy decision table, hotkey gesture state machine.
- **Integration tests** for the CLI and IPC socket (to do; the command
  parsing and protocol have unit tests).
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
| M4: Integrations | 9a (9b deferred) | CLI, URL scheme, Services, Continuity Camera |
| M5: Ship | 10 | Releases built, signed with a Developer ID, notarized and published by GitHub Actions, with Sparkle updates |
| M6: Beyond Yoink | Section 5 | In progress: pinning, quick actions, OCR, secret skipping, screenshots, auto-expiry and the keyboard workflow shipped in 0.2.0 to 0.3.0; see section 9 for the rest |

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

## 9. What's left (as of v0.6.1, 2026-10-12)

**Check on the installed app**
- The Shortcuts actions in the Shortcuts app, and the App Shortcuts
  phrases in Spotlight and Siri ("Show the Yoinks shelf"): only their
  metadata and the bridge are tested so far.
- The Share extension from real apps' Share menus (Safari, Photos, Mail);
  it was tested through the share services API, not the menus.
- Shake to show with a real drag (the detection has unit tests only).
- Save energy on battery while actually on battery (only the power
  source reading and the tick skipping are tested).
- The notch drop with a real drag, and with the built-in display next to
  (or not the main) external display: the side areas beside the notch
  are assumed to be in global screen coordinates, which could only be
  checked with the built-in display as the main one.
- Reordering while a search filters the shelf (the drop goes before the
  visible tile under the pointer; only the unfiltered case was tried).
- Windows that aren't Yoinks' own (Sparkle's update windows, the delete
  confirmation, the folder picker) on another desktop: Preferences,
  Clipboard History and the naming prompts follow the active desktop
  since 0.6.1, but these were not tried there.

**Features**
- The widget (recent shelf items or clips). Needs an App Group with the
  team and can only be tried from an installed, signed build.
- Text snippet editing: edit a text clipping in place before dragging it
  out.
- Custom actions on items, as in Dropzone and Dropover (partly covered by
  the Shortcuts actions and the command line).

**Testing**
- Integration tests for the CLI and IPC socket.
- The manual QA pass per release (section 6): Finder, Safari, Mail,
  Photos, Notes, full-screen apps, several displays, Spaces, Stage
  Manager, external volumes.
- An Instruments pass for idle CPU and memory, now that the clipboard,
  folder, expiry and power timers all run.

**Deferred, low priority**
- Dropping colors (Phase 2).
- A "show in Dock" toggle and adjustable gesture timing (Phase 8).
- Cloud uploads with share links and syncing shelves across Macs
  (section 8): both need a server or iCloud, and Yoinks otherwise makes
  no network calls except the update check.
