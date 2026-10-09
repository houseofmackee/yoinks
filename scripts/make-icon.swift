// Draws the Yoinks app icon and writes macos/AppIcon.icns.
//
// Usage (from the repository root): swift scripts/make-icon.swift
// Needs Xcode's command line tools (swift, iconutil).
import AppKit

let canvas: CGFloat = 1024
// macOS icon grid: an 824 pt rounded square centered on a 1024 pt canvas.
let body = NSRect(x: 100, y: 100, width: 824, height: 824)

func drawIcon(pixels: Int) -> Data {
    let rep = NSBitmapImageRep(
        bitmapDataPlanes: nil, pixelsWide: pixels, pixelsHigh: pixels,
        bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
        colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
    rep.size = NSSize(width: canvas, height: canvas)
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)

    // Soft shadow under the tile.
    let shadow = NSShadow()
    shadow.shadowColor = NSColor.black.withAlphaComponent(0.28)
    shadow.shadowOffset = NSSize(width: 0, height: -12)
    shadow.shadowBlurRadius = 28
    shadow.set()
    let tile = NSBezierPath(roundedRect: body, xRadius: 185, yRadius: 185)
    NSColor.black.setFill()
    tile.fill()
    NSShadow().set()

    // Blue to indigo gradient.
    let gradient = NSGradient(colors: [
        NSColor(srgbRed: 0.27, green: 0.62, blue: 1.00, alpha: 1),
        NSColor(srgbRed: 0.33, green: 0.30, blue: 0.93, alpha: 1),
    ])!
    gradient.draw(in: tile, angle: -90)

    // White tray with a down arrow.
    let config = NSImage.SymbolConfiguration(pointSize: 420, weight: .semibold)
        .applying(.init(paletteColors: [.white]))
    if let symbol = NSImage(systemSymbolName: "tray.and.arrow.down.fill", accessibilityDescription: nil)?
        .withSymbolConfiguration(config) {
        let size = symbol.size
        let origin = NSPoint(x: (canvas - size.width) / 2, y: (canvas - size.height) / 2 - 10)
        symbol.draw(in: NSRect(origin: origin, size: size))
    }

    NSGraphicsContext.restoreGraphicsState()
    return rep.representation(using: .png, properties: [:])!
}

let fm = FileManager.default
let iconset = URL(fileURLWithPath: NSTemporaryDirectory()).appendingPathComponent("AppIcon.iconset")
try? fm.removeItem(at: iconset)
try! fm.createDirectory(at: iconset, withIntermediateDirectories: true)
for points in [16, 32, 128, 256, 512] {
    for scale in [1, 2] {
        let name = scale == 1 ? "icon_\(points)x\(points).png" : "icon_\(points)x\(points)@2x.png"
        try! drawIcon(pixels: points * scale).write(to: iconset.appendingPathComponent(name))
    }
}
let output = "macos/AppIcon.icns"
let iconutil = Process()
iconutil.executableURL = URL(fileURLWithPath: "/usr/bin/iconutil")
iconutil.arguments = ["-c", "icns", iconset.path, "-o", output]
try! iconutil.run()
iconutil.waitUntilExit()
precondition(iconutil.terminationStatus == 0, "iconutil failed")
try? drawIcon(pixels: 1024).write(to: URL(fileURLWithPath: NSTemporaryDirectory()).appendingPathComponent("AppIcon-preview.png"))
print("wrote \(output)")
