// Draws assets/AppIcon.iconset: the motif (Avid database + check) on the dark plate of MediaFiles Ferry / Mountpoint.
// Every size is drawn for itself (bigger plate and thicker lines when small), not scaled down from 1024.
// Run: swift scripts/make-icon.swift   (from the repo root)
import AppKit

let out = URL(fileURLWithPath: "assets/AppIcon.iconset")
try? FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)

func draw(px: Int) -> Data {
    let s = CGFloat(px)
    let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: px, pixelsHigh: px, bitsPerSample: 8, samplesPerPixel: 4,
                               hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
    let c = NSGraphicsContext.current!.cgContext
    // design units: 1024, y down
    c.translateBy(x: 0, y: s)
    c.scaleBy(x: s / 1024, y: -s / 1024)
    // plate: Apple grid (824 of 1024); small sizes get a bigger plate (16 px → 14 px)
    let margin: CGFloat = px <= 16 ? 64 : px <= 32 ? 64 : 100
    let plate = CGRect(x: margin, y: margin, width: 1024 - 2 * margin, height: 1024 - 2 * margin)
    let radius = plate.width * 0.225
    let path = CGPath(roundedRect: plate, cornerWidth: radius, cornerHeight: radius, transform: nil)
    c.saveGState()
    if px > 32 { c.setShadow(offset: CGSize(width: 0, height: -10 * 1024 / s * s / 1024), blur: 20, color: NSColor(white: 0, alpha: 0.35).cgColor) }
    c.addPath(path); c.setFillColor(NSColor(srgbRed: 0x40/255, green: 0x44/255, blue: 0x4a/255, alpha: 1).cgColor); c.fillPath()
    c.restoreGState()
    c.saveGState(); c.addPath(path); c.clip()
    let grad = CGGradient(colorsSpace: CGColorSpaceCreateDeviceRGB(), colors: [NSColor(srgbRed: 0x4d/255, green: 0x53/255, blue: 0x5a/255, alpha: 1).cgColor, NSColor(srgbRed: 0x32/255, green: 0x35/255, blue: 0x3b/255, alpha: 1).cgColor] as CFArray, locations: [0, 1])!
    c.drawLinearGradient(grad, start: CGPoint(x: 0, y: plate.minY), end: CGPoint(x: 0, y: plate.maxY), options: [])
    c.restoreGState()
    if px >= 64 {
        c.addPath(CGPath(roundedRect: plate.insetBy(dx: 1.5, dy: 1.5), cornerWidth: radius - 1.5, cornerHeight: radius - 1.5, transform: nil))
        c.setStrokeColor(NSColor(white: 1, alpha: 0.10).cgColor); c.setLineWidth(3); c.strokePath()
    }

    // motif, centred on the plate; lines get thicker when the icon gets small
    let k = plate.width / 824
    c.translateBy(x: 512, y: 512); c.scaleBy(x: k, y: k); c.translateBy(x: -535, y: -537)
    let w: CGFloat = px <= 16 ? 70 : px <= 32 ? 52 : px <= 64 ? 44 : 34
    let light = NSColor(srgbRed: 0xec/255, green: 0xee/255, blue: 0xf0/255, alpha: 1).cgColor
    c.setStrokeColor(light); c.setLineWidth(w); c.setLineCap(.round); c.setLineJoin(.round)
    // database: top ellipse, sides, rings (one ring when tiny)
    let (cx, rx, ry, top, bottom): (CGFloat, CGFloat, CGFloat, CGFloat, CGFloat) = (440, 170, 56, 330, 650)
    c.strokeEllipse(in: CGRect(x: cx - rx, y: top - ry, width: 2 * rx, height: 2 * ry))
    let body = CGMutablePath()
    body.move(to: CGPoint(x: cx - rx, y: top)); body.addLine(to: CGPoint(x: cx - rx, y: bottom))
    body.addArc(center: CGPoint(x: cx, y: bottom), radius: rx, startAngle: .pi, endAngle: 0, clockwise: true, transform: CGAffineTransform(translationX: cx, y: bottom).scaledBy(x: 1, y: ry / rx).translatedBy(x: -cx, y: -bottom))
    body.addLine(to: CGPoint(x: cx + rx, y: top))
    c.addPath(body); c.strokePath()
    for y in (px <= 16 ? [490] : [437, 544]) as [CGFloat] {
        let ring = CGMutablePath()
        ring.addArc(center: CGPoint(x: cx, y: y), radius: rx, startAngle: .pi, endAngle: 0, clockwise: true, transform: CGAffineTransform(translationX: cx, y: y).scaledBy(x: 1, y: ry / rx).translatedBy(x: -cx, y: -y))
        c.addPath(ring); c.strokePath()
    }
    // check badge: punched out of the database, ring, tick
    let badge = CGRect(x: 660 - 140, y: 660 - 140, width: 280, height: 280)
    c.setFillColor(NSColor(srgbRed: 0x3a/255, green: 0x3e/255, blue: 0x44/255, alpha: 1).cgColor)
    c.fillEllipse(in: badge.insetBy(dx: -w / 2, dy: -w / 2))
    c.strokeEllipse(in: badge)
    let tick = CGMutablePath()
    tick.move(to: CGPoint(x: 598, y: 662)); tick.addLine(to: CGPoint(x: 649, y: 713)); tick.addLine(to: CGPoint(x: 751, y: 602))
    c.setLineWidth(w * 1.1); c.addPath(tick); c.strokePath()

    NSGraphicsContext.current = nil
    return rep.representation(using: .png, properties: [:])!
}

for base in [16, 32, 128, 256, 512] {
    try! draw(px: base).write(to: out.appendingPathComponent("icon_\(base)x\(base).png"))
    try! draw(px: base * 2).write(to: out.appendingPathComponent("icon_\(base)x\(base)@2x.png"))
}
try! draw(px: 1024).write(to: URL(fileURLWithPath: "assets/icon.png"))
