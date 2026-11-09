// Icons for the settings tabs and the self-test list (same SF Symbols as Mountpoint), white on transparent;
// Slint tints them with `colorize`. Run: swift scripts/make-tab-icons.swift
import AppKit

let icons = ["general": "gearshape", "updates": "arrow.triangle.2.circlepath", "support": "lifepreserver", "about": "info.circle", "rebalance": "rectangle.split.3x1",
             "pass": "checkmark.circle.fill", "fail": "xmark.circle.fill", "skip": "minus.circle"]
for (name, symbol) in icons {
    let config = NSImage.SymbolConfiguration(pointSize: 40, weight: .regular)
    guard let image = NSImage(systemSymbolName: symbol, accessibilityDescription: nil)?.withSymbolConfiguration(config) else { fatalError(symbol) }
    let size = 56
    let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: size, pixelsHigh: size, bitsPerSample: 8, samplesPerPixel: 4,
                               hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
    let s = image.size, scale = CGFloat(size) / max(s.width, s.height)
    let rect = NSRect(x: (CGFloat(size) - s.width * scale) / 2, y: (CGFloat(size) - s.height * scale) / 2, width: s.width * scale, height: s.height * scale)
    image.draw(in: rect)
    NSColor.white.set()
    rect.fill(using: .sourceAtop)
    NSGraphicsContext.restoreGraphicsState()
    try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: "assets/tabs/\(name).png"))
}
