# Yoinks

A drag-and-drop shelf for macOS, written in Rust and inspired by
[Yoink](https://eternalstorms.at/yoink/mac/). Park files, text, links and
images on a shelf at the edge of the screen while you move between windows,
Spaces and full-screen apps, then drag them out where you need them.

Requires macOS 13 or later on Apple Silicon.

## Features

- **Shelf that appears when you drag.** Start dragging anything and the shelf
  slides in at the screen edge (or near the cursor, or only when you approach
  the edge). It hides again if you don't use it. With items on it, it stays
  until you hide it; once it's empty again it slides away by itself.
- **Put it anywhere.** Drag the shelf by its background to move it; it comes
  back to that spot. "Move Shelf Back to Edge" undoes it.
- **Holds almost anything.** Files (as references, never copied), Mail and
  Photos attachments, text, rich text, links and images. Several items
  dropped together become a stack.
- **Drag out with Finder rules.** Same volume moves, another volume copies,
  Option copies, Command moves. Text and links drop into text fields and
  editors as text.
- **Recall.** The last 10 items you dragged out can be brought back, even if
  Finder moved them.
- **Keyboard shortcut** (default ⌃⌥Y): press to show or hide the shelf, hold
  to recall the last item, press twice to add the clipboard.
- **Quick Look and thumbnails.** Space previews the selected item; arrows,
  Return and Delete work too.
- **Clipboard history** from the menu bar, with search, pins, and privacy
  filters: password managers and content marked as concealed are never
  recorded.
- **Command line, links and Services.** `yoinks add file.pdf`,
  `yoinks://add?text=...`, and "Add to Yoinks" in the Services menu and
  Finder's Quick Actions.
- **Preferences** for all of the above, applied instantly.

## Install

1. Download `Yoinks-<version>.dmg` from
   [Releases](https://github.com/houseofmackee/yoinks/releases) (the current
   pre-release is
   [0.1.3-beta.1](https://github.com/houseofmackee/yoinks/releases/tag/v0.1.3-beta.1)), open
   it and drag Yoinks into Applications. To build it yourself, see
   [Building](#building).
2. Releases are signed with the project's self-signed "Yoinks Release"
   certificate but not notarized, so macOS blocks the first launch.
   Open Yoinks once, then go to System Settings > Privacy & Security and
   choose **Open Anyway**.

Yoinks lives in the menu bar (the tray icon); it has no Dock icon. You can
hide the menu bar icon in Preferences: the same menu (Preferences, Clipboard
History, Quit, ...) is always available from the gear button at the bottom
of the shelf, and opening Yoinks again brings up the shelf.

## Using Yoinks

| To | Do |
| --- | --- |
| Put something on the shelf | Drag it to the shelf, or press ⌃⌥Y twice to add the clipboard |
| Take it off | Drag it out, or right-click > Remove |
| Show or hide the shelf | ⌃⌥Y, the menu bar icon, or Escape |
| Bring back what you dragged out | Hold ⌃⌥Y, or menu bar > Recall Last Dragged Item |
| Preview | Select a tile and press Space |
| Split a stack | Right-click > Unstack |
| Move the shelf | Drag its background (between or below the tiles) |
| Put it back at the edge | Menu bar or gear > Move Shelf Back to Edge |
| Clipboard history | Menu bar (or the shelf's gear) > Clipboard History… (type to search, Return copies) |
| Preferences | Menu bar or the shelf's gear button > Preferences… |

### Command line

The app binary doubles as a command line tool. To use it from a shell, link
it somewhere on your `PATH`, for example:

```sh
ln -s /Applications/Yoinks.app/Contents/MacOS/yoinks ~/.local/bin/yoinks
```

```sh
yoinks add report.pdf notes.txt     # one stack
yoinks add --text "Call Sam at 3"
yoinks add --url https://example.com --title "Example"
yoinks list                          # or: yoinks list --json
yoinks show | hide | recall | clear
```

Yoinks is started in the background if it is not running.

### Links

`yoinks://add?path=/absolute/path` (repeat `path` for a stack),
`yoinks://add?text=...`, `yoinks://add?url=...&title=...`,
`yoinks://show`, `yoinks://hide`, `yoinks://recall`. Values are
percent-encoded. Links can't clear the shelf.

## Settings and data

Everything is in `~/Library/Application Support/Yoinks`:

| Path | Contents |
| --- | --- |
| `settings.json` | Preferences (editable by hand; restart Yoinks after editing) |
| `shelf.json`, `Items/` | The shelf, and copies of dropped text, images and attachments |
| `clipboard.json`, `Clips/` | Clipboard history |
| `Thumbnails/` | Cached previews |

Set `YOINKS_DATA_DIR` to use a different folder (handy for testing).

Nothing leaves your Mac: there is no network access, analytics or account.

## Building

Needs Rust (stable) and Xcode's command line tools.

```sh
cargo test                     # unit tests
scripts/bundle.sh              # target/debug/Yoinks.app
scripts/bundle.sh release      # target/release/Yoinks.app
scripts/make-dmg.sh            # target/release/Yoinks-<version>.dmg
scripts/dist.sh                # target/dist: DMG, zip and SHA256SUMS.txt
swift scripts/make-icon.swift  # regenerate macos/AppIcon.icns
```

`bundle.sh` registers the app with Launch Services so the link scheme and the
service work during development (set `YOINKS_SKIP_REGISTER=1` to skip). Debug
builds are signed ad hoc; release builds use the "Yoinks Release" certificate
if it is in the keychain, otherwise ad hoc (`SIGN_IDENTITY` overrides).

GitHub Actions runs `cargo fmt --check`, `cargo clippy -D warnings` (debug and
release), `cargo test`, a release bundle build and `cargo deny check` (see
`deny.toml`) on every push to master and every pull request.

### Releasing

Releases are built and published by GitHub Actions, not from a local machine.
Bump the version in `Cargo.toml`, commit, then push a matching tag:

```sh
git tag v0.2.0 && git push origin master v0.2.0
```

`.github/workflows/release.yml` checks the tag matches `Cargo.toml`, signs
with the "Yoinks Release" certificate from the repository secrets
`RELEASE_CERT_P12_BASE64` and `RELEASE_CERT_PASSWORD` (made once with
`scripts/create-release-cert.sh`), runs `scripts/dist.sh` and publishes the
DMG, zip and `SHA256SUMS.txt`. Tags with a hyphen (`v0.2.0-beta.1`) become
pre-releases. Do not create releases by hand with `gh release create`.

## Status

See [PLAN.md](PLAN.md) for the roadmap. Still to do: Developer ID signing and
notarization, automatic updates, and optional Swift add-ons (Share
extension, Shortcuts actions, widget).
