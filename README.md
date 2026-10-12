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
  Return, Delete and ⌘C work too, and ⌘V adds the clipboard.
- **Search.** Start typing on the shelf (or press ⌘F) to narrow it down by
  name, link, kind or the text inside text items.
- **Quick actions.** Right-click a tile for Open With, Copy, Copy Path,
  Rename (text, links and images Yoinks saved), Compress to zip, Copy Text
  (recognizes the text in an image), and Convert or Resize images. Results
  land on the shelf as new items; the originals are never changed.
- **Pinning.** Pinned items stay on the shelf after you drag them out and
  survive Clear Shelf.
- **Screenshots and auto-expiry** (both optional): new screenshots and
  screen recordings appear on the shelf, and unpinned items can be removed
  after an hour, a day, a week or 30 days.
- **Automatic updates** through [Sparkle](https://sparkle-project.org):
  on the second launch Yoinks asks whether to check automatically (once a
  day); change it later in Preferences > General, or choose Check for
  Updates… from the menu. Pre-releases are not offered as updates.
- **Clipboard history** from the menu bar, with search, pins, and privacy
  filters: password managers and content marked as concealed are never
  recorded, and likely passwords, API keys and tokens are skipped (this can
  be turned off).
- **Command line, links and Services.** `yoinks add file.pdf`,
  `yoinks://add?text=...`, and "Add to Yoinks" in the Services menu and
  Finder's Quick Actions.
- **Preferences** for all of the above, applied instantly.

## Install

1. Download `Yoinks-<version>.dmg` from
   [Releases](https://github.com/houseofmackee/yoinks/releases) (the current
   release is
   [0.3.0](https://github.com/houseofmackee/yoinks/releases/tag/v0.3.0)), open
   it and drag Yoinks into Applications. To build it yourself, see
   [Building](#building).
2. Releases after 0.2.0-beta.1 are signed with a Developer ID and notarized
   by Apple, so Yoinks opens like any other app. Earlier releases were
   self-signed: macOS blocks their first launch until you choose **Open
   Anyway** in System Settings > Privacy & Security.

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
| Keep an item after dragging it out | Right-click > Pin |
| Copy it, rename it, zip it, convert an image | Right-click > Copy, Rename…, Compress, Convert Image |
| Get the text out of an image | Right-click > Copy Text |
| Paste onto the shelf | Click the shelf and press ⌘V |
| Find something on the shelf | Click the shelf and start typing, or press ⌘F; Escape ends the search |
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

No account, analytics or crash reporting. The only network access is the
update check, if you allow it: it fetches `appcast.xml` from GitHub
Releases and sends nothing about your Mac (Sparkle's system profiling is
off).

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
builds are signed ad hoc; release builds use a Developer ID Application
identity if one is in the keychain (`YOINKS_TEAM_ID` picks the team), then
the self-signed "Yoinks Release" certificate, otherwise ad hoc
(`SIGN_IDENTITY` overrides). With a Developer ID, `dist.sh` also notarizes
and staples the app and the DMG when `YOINKS_NOTARY_KEY` (the `.p8` path),
`YOINKS_NOTARY_KEY_ID` and `YOINKS_NOTARY_ISSUER` are set.

Release bundles embed Sparkle 2, downloaded once by `scripts/sparkle.sh` at
a pinned version and checksum (`YOINKS_NO_SPARKLE=1` leaves it out). With
`YOINKS_SPARKLE_KEY` (the path to the EdDSA private key) `dist.sh` also
writes a signed `appcast.xml`, which the app reads from the latest
release. `CFBundleVersion` is the version without the hyphen and dot
(`0.2.0beta2` for `0.2.0-beta.2`, see `scripts/version.sh`), because
Sparkle compares that and would otherwise treat all of 0.2.0's
pre-releases and 0.2.0 itself as the same version. Back the key up outside
GitHub: without it, installed copies can't verify new updates. To try an
update locally, run a release build with `YOINKS_UPDATE_TEST=1` and
`YOINKS_FEED_URL` pointing at a test feed signed with the same key.

GitHub Actions runs `cargo fmt --check`, `cargo clippy -D warnings` (debug and
release), `cargo test`, a release bundle build and `cargo deny check` (see
`deny.toml`) on every push to master and every pull request.

### Releasing

Releases are built and published by GitHub Actions, not from a local machine.
Move the `[Unreleased]` entries in `CHANGELOG.md` under a new
`## [x.y.z] (date)` heading, bump the version in `Cargo.toml`, commit, then
push a matching tag:

```sh
git tag v0.2.0 && git push origin master v0.2.0
```

`.github/workflows/release.yml` checks the tag matches `Cargo.toml`, signs
with the Developer ID Application certificate from the repository secrets
`DEVELOPER_ID_P12_BASE64` and `DEVELOPER_ID_P12_PASSWORD`, notarizes with
the App Store Connect API key in `NOTARY_API_KEY_BASE64` (and the
`APPLE_TEAM_ID`, `NOTARY_KEY_ID` and `NOTARY_ISSUER_ID` variables), signs
the update feed with `SPARKLE_PRIVATE_KEY`, uses the version's
`CHANGELOG.md` section (`scripts/release-notes.py`) as the notes in the
update window and on the GitHub release, runs `scripts/dist.sh` and
publishes the DMG, zip, `SHA256SUMS.txt` and `appcast.xml`. Keep backups
of the `.p12`, `.p8` and Sparkle key outside GitHub. Tags with a hyphen
(`v0.2.0-beta.1`) become pre-releases; installed copies only update to
full releases, because the feed is read from the latest one. Do not create
releases by hand with `gh release create`.

## Status

See [PLAN.md](PLAN.md) for the roadmap and [CHANGELOG.md](CHANGELOG.md) for
what changed in each version. Still to do: optional Swift add-ons (Share
extension, Shortcuts actions, widget), named shelves and text editing.

Release builds include [Sparkle](https://sparkle-project.org) for updates,
under its own MIT-style license; the notice ships in the app as
`Contents/Resources/Sparkle-LICENSE.txt`.
