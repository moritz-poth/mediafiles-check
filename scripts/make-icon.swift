// Draws the app icon for every size on Apple's icon grid, in Mountpoint's style (small sizes have less margin).
//   swift scripts/make-icon.swift            → assets/AppIcon.iconset (all sizes, used by scripts/build-mac-app.sh) + assets/icon.png
//   swift scripts/make-icon.swift <dir>      → only the iconset sizes into <dir>
// Motif "database + check" (concept A, chosen 2026-11-09): solid light database with seam rings, green check badge
// punched out of its lower right. Geometry in a 256-unit grid like Mountpoint's make-icon.swift (same plate, colours,
// motif scale and green #1a7f37).
import AppKit

/// Plate size per pixel size, measured from macOS system app icons (same table as Mountpoint).
let plateFor: [Int: Int] = [16: 14, 32: 28, 64: 52, 128: 104, 256: 206, 512: 412, 1024: 824]

func color(_ hex: UInt32, _ a: CGFloat = 1) -> CGColor {
    CGColor(red: CGFloat(hex >> 16 & 0xff) / 255, green: CGFloat(hex >> 8 & 0xff) / 255, blue: CGFloat(hex & 0xff) / 255, alpha: a)
}

/// Half ellipse from the left to the right end; `top` = back edge (through the top), else front edge (through the bottom).
func halfEllipse(_ p: CGMutablePath, cx: CGFloat, cy: CGFloat, rx: CGFloat, ry: CGFloat, top: Bool, leftToRight: Bool = true) {
    let t = CGAffineTransform(translationX: cx, y: cy).scaledBy(x: 1, y: ry / rx)
    // y points down here: angle π = left, 3π/2 = top, π/2 = bottom
    let (a, b): (CGFloat, CGFloat) = top ? (.pi, 2 * .pi) : (.pi, 0)
    let (s, e) = leftToRight ? (a, b) : (b, a)
    p.addArc(center: .zero, radius: rx, startAngle: s, endAngle: e, clockwise: top ? !leftToRight : leftToRight, transform: t)
}

func render(_ size: Int) -> Data {
    let plateSize = CGFloat(plateFor[size]!)
    let margin = (CGFloat(size) - plateSize) / 2
    let k = plateSize / 824   // everything is designed for the 824 px plate
    let small = size <= 32

    let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: size, pixelsHigh: size, bitsPerSample: 8,
                               samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
                               bytesPerRow: 0, bitsPerPixel: 0)!
    NSGraphicsContext.saveGraphicsState()
    let ctx = NSGraphicsContext(bitmapImageRep: rep)!
    NSGraphicsContext.current = ctx
    let cg = ctx.cgContext
    cg.translateBy(x: 0, y: CGFloat(size))   // origin top left
    cg.scaleBy(x: 1, y: -1)

    // plate – identical to Mountpoint
    let plateRect = CGRect(x: margin, y: margin, width: plateSize, height: plateSize)
    let radius = 185 * k
    let plate = CGPath(roundedRect: plateRect, cornerWidth: radius, cornerHeight: radius, transform: nil)
    cg.saveGState()
    if size >= 64 { cg.setShadow(offset: CGSize(width: 0, height: -10 * k), blur: 20 * k, color: color(0x000000, 0.35)) }
    cg.addPath(plate)
    cg.setFillColor(color(0x2e3236))
    cg.fillPath()
    cg.restoreGState()
    cg.saveGState()
    cg.addPath(plate)
    cg.clip()
    let gradient = CGGradient(colorsSpace: CGColorSpaceCreateDeviceRGB(),
                              colors: [color(0x3c4146), color(0x272a2e)] as CFArray, locations: [0, 1])!
    cg.drawLinearGradient(gradient, start: CGPoint(x: 0, y: plateRect.minY), end: CGPoint(x: 0, y: plateRect.maxY), options: [])
    cg.restoreGState()
    let edge = max(3 * k, 0.75)
    cg.addPath(CGPath(roundedRect: plateRect.insetBy(dx: edge / 2, dy: edge / 2),
                      cornerWidth: radius - edge / 2, cornerHeight: radius - edge / 2, transform: nil))
    cg.setStrokeColor(color(0xffffff, 0.10))
    cg.setLineWidth(edge)
    cg.strokePath()

    // motif: same scale and optical offset as Mountpoint; tiny sizes get a larger motif
    let boost: CGFloat = size <= 16 ? 1.3 : size <= 32 ? 1.18 : size <= 64 ? 1.08 : 1
    let unit = k * 4 * 0.62 * boost
    cg.translateBy(x: margin + 412 * k, y: margin + 412 * k + 16 * k)
    cg.scaleBy(x: unit, y: unit)
    cg.translateBy(x: -128, y: -138)

    let (cx, rx, ry, top, bottom): (CGFloat, CGFloat, CGFloat, CGFloat, CGFloat) = (128, 76, 22, 62, 202)
    // badge: bigger at small sizes, otherwise the check turns into a green dot
    let badge = small ? CGPoint(x: 194, y: 188) : CGPoint(x: 198, y: 192)
    let badgeR: CGFloat = small ? 56 : 46
    let gap: CGFloat = small ? 12 : 10

    // database in its own layer, so the badge gap is a real hole showing the plate
    cg.beginTransparencyLayer(auxiliaryInfo: nil)
    let body = CGMutablePath()
    body.move(to: CGPoint(x: cx - rx, y: top))
    halfEllipse(body, cx: cx, cy: top, rx: rx, ry: ry, top: true)
    body.addLine(to: CGPoint(x: cx + rx, y: bottom))
    halfEllipse(body, cx: cx, cy: bottom, rx: rx, ry: ry, top: false, leftToRight: false)
    body.closeSubpath()
    cg.addPath(body)
    cg.setFillColor(color(0xe9eaeb))
    cg.fillPath()
    cg.addPath(body)   // the stroke rounds the corners, like Mountpoint's solid motif
    cg.setStrokeColor(color(0xe9eaeb))
    cg.setLineWidth(12)
    cg.setLineJoin(.round)
    cg.strokePath()
    // seams: the top rim always (it makes the cylinder), the two rings only where they are still visible
    let seams = CGMutablePath()
    for y in (size >= 64 ? [top, 109, 156] : [top]) as [CGFloat] {
        seams.move(to: CGPoint(x: cx - rx - 6, y: y))
        halfEllipse(seams, cx: cx, cy: y, rx: rx + 6, ry: ry, top: false)   // + 6 = to the edge of the rounding stroke
    }
    cg.addPath(seams)
    cg.setStrokeColor(color(0x272a2e, small ? 0.45 : 0.22))
    cg.setLineWidth(small ? 12 : 6)
    cg.strokePath()
    cg.setBlendMode(.clear)
    cg.fillEllipse(in: CGRect(x: badge.x - badgeR - gap, y: badge.y - badgeR - gap, width: 2 * (badgeR + gap), height: 2 * (badgeR + gap)))
    cg.setBlendMode(.normal)
    cg.endTransparencyLayer()

    // green check badge
    cg.setFillColor(color(0x1a7f37))
    cg.fillEllipse(in: CGRect(x: badge.x - badgeR, y: badge.y - badgeR, width: 2 * badgeR, height: 2 * badgeR))
    let s = badgeR / 40   // tick designed for a radius of 40
    let tick = CGMutablePath()
    tick.move(to: CGPoint(x: badge.x - 20 * s, y: badge.y + 1 * s))
    tick.addLine(to: CGPoint(x: badge.x - 6 * s, y: badge.y + 15 * s))
    tick.addLine(to: CGPoint(x: badge.x + 22 * s, y: badge.y - 15 * s))
    cg.addPath(tick)
    cg.setStrokeColor(color(0xffffff))
    cg.setLineWidth(small ? 16 * s : 13 * s)
    cg.setLineCap(.round)
    cg.setLineJoin(.round)
    cg.strokePath()

    NSGraphicsContext.restoreGraphicsState()
    return rep.representation(using: .png, properties: [:])!
}

func writeIconset(_ dir: String) {
    try! FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
    for s in [16, 32, 128, 256, 512] {
        try! render(s).write(to: URL(fileURLWithPath: "\(dir)/icon_\(s)x\(s).png"))
        try! render(s * 2).write(to: URL(fileURLWithPath: "\(dir)/icon_\(s)x\(s)@2x.png"))
    }
}

if let dir = CommandLine.arguments.dropFirst().first {
    writeIconset(dir)
    print(dir)
} else {
    writeIconset("assets/AppIcon.iconset")
    try! render(1024).write(to: URL(fileURLWithPath: "assets/icon.png"))
    print("assets/AppIcon.iconset, assets/icon.png")
}
