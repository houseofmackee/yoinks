# Changelog

All notable changes to Yoinks. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/). Each version's section is also
shown in the update window and on its GitHub release.

## [Unreleased]

### Fixed

- The shelf could get stuck on one desktop and only open there. It now
  always opens on the desktop you are on, and an open shelf comes along
  when you switch desktops (the notch drop target too).

## [0.6.1] (2026-10-12)

### Changed

- Preferences has a new Adding tab (notch, screenshots, watched folders),
  so the Shelf tab fits on smaller screens again.

### Fixed

- Preferences, Clipboard History and the naming prompts open on the
  desktop you are on, not the one where they were first opened.

## [0.6.0] (2026-10-12)

### Added

- Reorder the shelf: drag a tile up or down; a line shows where it will
  go.
- Drop onto the menu bar icon to add to the shelf, without waiting for the
  shelf to appear.
- Drop onto the notch (Macs with one): it opens into a "Drop to add to
  Yoinks" pill. On by default; Preferences > Shelf turns it off.

## [0.5.0] (2026-10-12)

### Added

- Homebrew: `brew install --cask houseofmackee/tap/yoinks` installs the app
  and the `yoinks` command; full releases update the cask.
- Named shelves: Shelves in the menu creates, renames, deletes and
  switches between them (⌘1 to ⌘9 on the shelf too). Each can have a
  Finder-style color tag, shown next to its name at the top of the shelf
  and, optionally, as a tint of the shelf's background (Preferences >
  Shelf).
- Share: right-click a tile to send it with AirDrop, Mail, Messages,
  Notes and the other share services.
- Shake to show: a new "When you shake the pointer" choice for showing the
  shelf during drags (Preferences > Shelf).
- Watched folders: new files and folders in the folders you pick (for
  example Downloads) are added to the shelf once they have finished
  copying or downloading (Preferences > Shelf).
- Shortcuts actions: Add Files, Add Text, Add Link, Get Shelf Items,
  Show Shelf, Hide Shelf, Clear Shelf, Bring Back Last Item and Switch
  Shelf.
- "Yoinks" in other apps' Share menus (Safari, Photos, Finder and more):
  shared files, images, links and text go on the shelf. It may need to
  be switched on once in System Settings > General > Login Items &
  Extensions > Sharing.
- `yoinks shelf <name>` switches shelves; `yoinks shelf` lists them.

### Fixed

- The command line no longer makes Yoinks log a "Broken pipe" error
  each time it checks whether the app is running.

## [0.4.0] (2026-10-12)

### Added

- Search the shelf: start typing on the shelf, or press ⌘F (also in the
  shelf's right-click menu). Every word must match a name, link, kind
  (file, text, image, link) or the text inside a text item. Down or
  Return moves to the first match; Escape ends the search, and hiding the
  shelf clears it. Search Shelf is also in the menu bar and gear menus.
- Save energy on battery (Preferences > General, off by default): on
  battery or in Low Power Mode, Yoinks checks the clipboard every 2 s
  instead of every 0.5 s, the screenshot folder every 5 s, and makes no
  new thumbnails until the Mac is plugged in.

### Changed

- Preferences has a new Shelf tab for the shelf's options (edge,
  appearance, auto-hide, expiry, screenshots, ignored apps), so General
  is short again.
- Copy Text sits next to Copy and Copy Path in a tile's menu.

## [0.3.0] (2026-10-12)

### Added

- Automatic updates through Sparkle: Check for Updates… in the menu, and
  optional daily checks (asked on the second launch; Preferences >
  General). Updates are verified with an EdDSA signature before they are
  installed. Pre-releases are not offered as updates.
- Copy Text: right-click an image to copy the text in it (Vision).
- Optional: new screenshots and screen recordings are added to the shelf.
- Optional: unpinned items are removed after 1 hour to 30 days.
- ⌘V on the shelf adds what's on the clipboard.

### Changed

- Releases are signed with a Developer ID and notarized by Apple, so
  Yoinks opens without the "Open Anyway" step.

## [0.2.0-beta.1] (2026-10-11)

### Added

- Quick actions on tiles: Open With, Copy (⌘C), Copy Path, Rename (items
  Yoinks saved), Compress, and Convert or Resize images. Results are new
  items; originals are never changed.
- Pin items so they stay after being dragged out and survive Clear Shelf.
- Clipboard history skips likely passwords, API keys and tokens (can be
  turned off in Preferences > Clipboard).

### Fixed

- ⌘A, ⌘C, ⌘V, ⌘X and ⌘Z work in text fields.

## [0.1.3-beta.1] (2026-10-11)

### Changed

- Releases are built, signed and published by GitHub Actions.
- The Preferences window title shows the version.

## [0.1.2] (2026-10-10)

### Changed

- The shelf stays while it has items and slides away once it is empty.

## [0.1.1] (2026-10-10)

### Added

- First release: the drag-and-drop shelf, automatic appearance during
  drags, the keyboard shortcut, Quick Look, clipboard history,
  Preferences, the `yoinks` command, `yoinks://` links and the Services
  menu.
