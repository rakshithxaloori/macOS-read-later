// Draws the 1024 px Read Later app icon: an ivory open book with a copper
// ribbon on deep green, following the macOS icon grid. Run via make-icon.sh.
import AppKit

let size = 1024.0
let out = CommandLine.arguments[1]
let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: Int(size), pixelsHigh: Int(size),
                           bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
                           colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
let cg = NSGraphicsContext.current!.cgContext
// Work in top-left coordinates like the design sketch.
cg.translateBy(x: 0, y: size)
cg.scaleBy(x: 1, y: -1)
func rgb(_ r: Double, _ g: Double, _ b: Double, _ a: Double = 1) -> CGColor {
    CGColor(red: r / 255, green: g / 255, blue: b / 255, alpha: a)
}

// Tile: 824 px rounded square inset 100 px, with a soft drop shadow.
let tile = CGRect(x: 100, y: 100, width: 824, height: 824)
let tilePath = CGPath(roundedRect: tile, cornerWidth: 186, cornerHeight: 186, transform: nil)
cg.saveGState()
cg.setShadow(offset: CGSize(width: 0, height: -10), blur: 28, color: rgb(0, 0, 0, 0.30))
cg.addPath(tilePath); cg.setFillColor(rgb(35, 64, 47)); cg.fillPath()
cg.restoreGState()
cg.saveGState()
cg.addPath(tilePath); cg.clip()
let space = CGColorSpaceCreateDeviceRGB()
let green = CGGradient(colorsSpace: space, colors: [rgb(66, 108, 86), rgb(33, 61, 45)] as CFArray, locations: [0, 1])!
cg.drawLinearGradient(green, start: CGPoint(x: 512, y: 100), end: CGPoint(x: 512, y: 924), options: [])
// A faint warm glow behind the book.
let glow = CGGradient(colorsSpace: space, colors: [rgb(252, 240, 214, 0.20), rgb(252, 240, 214, 0)] as CFArray, locations: [0, 1])!
cg.drawRadialGradient(glow, startCenter: CGPoint(x: 512, y: 470), startRadius: 0, endCenter: CGPoint(x: 512, y: 470), endRadius: 420, options: [])
cg.restoreGState()
// Hairline highlight on the tile edge.
cg.addPath(CGPath(roundedRect: tile.insetBy(dx: 3, dy: 3), cornerWidth: 183, cornerHeight: 183, transform: nil))
cg.setStrokeColor(rgb(255, 255, 255, 0.10)); cg.setLineWidth(6); cg.strokePath()

// Open book: two gently curved pages meeting at the spine.
func page(_ dir: Double) -> CGPath {
    let p = CGMutablePath()
    p.move(to: CGPoint(x: 512, y: 372))
    p.addCurve(to: CGPoint(x: 512 + dir * 268, y: 334), control1: CGPoint(x: 512 + dir * 80, y: 318), control2: CGPoint(x: 512 + dir * 190, y: 316))
    p.addLine(to: CGPoint(x: 512 + dir * 268, y: 676))
    p.addCurve(to: CGPoint(x: 512, y: 716), control1: CGPoint(x: 512 + dir * 190, y: 658), control2: CGPoint(x: 512 + dir * 80, y: 662))
    p.closeSubpath()
    return p
}
cg.saveGState()
cg.setShadow(offset: CGSize(width: 0, height: 14), blur: 30, color: rgb(10, 25, 17, 0.45))
for dir in [-1.0, 1.0] { cg.addPath(page(dir)); cg.setFillColor(rgb(250, 246, 236)); cg.fillPath() }
cg.restoreGState()
// Slight shading toward the spine gives the pages depth.
for dir in [-1.0, 1.0] {
    cg.saveGState()
    cg.addPath(page(dir)); cg.clip()
    let shade = CGGradient(colorsSpace: space, colors: [rgb(214, 204, 182, 0.65), rgb(250, 246, 236, 0)] as CFArray, locations: [0, 1])!
    cg.drawLinearGradient(shade, start: CGPoint(x: 512, y: 500), end: CGPoint(x: 512 + dir * 110, y: 500), options: [])
    cg.restoreGState()
}
// Text lines on each page.
cg.setLineCap(.round)
cg.setLineWidth(11)
cg.setStrokeColor(rgb(176, 184, 170))
for dir in [-1.0, 1.0] {
    for i in 0..<5 {
        let y = 420.0 + Double(i) * 50
        let inner = 512 + dir * 66, outer = 512 + dir * (i == 4 ? 170 : 222)
        let sag = dir * 0
        cg.move(to: CGPoint(x: inner, y: y + 6 + sag)); cg.addLine(to: CGPoint(x: outer, y: y)); cg.strokePath()
    }
}
// Spine.
cg.setStrokeColor(rgb(196, 186, 162)); cg.setLineWidth(4)
cg.move(to: CGPoint(x: 512, y: 372)); cg.addLine(to: CGPoint(x: 512, y: 716)); cg.strokePath()

// Copper ribbon falling from the right page past its lower edge.
let ribbon = CGMutablePath()
ribbon.move(to: CGPoint(x: 640, y: 330))
ribbon.addLine(to: CGPoint(x: 700, y: 326))
ribbon.addLine(to: CGPoint(x: 700, y: 780))
ribbon.addLine(to: CGPoint(x: 670, y: 748))
ribbon.addLine(to: CGPoint(x: 640, y: 780))
ribbon.closeSubpath()
cg.saveGState()
cg.setShadow(offset: CGSize(width: 0, height: 6), blur: 10, color: rgb(40, 20, 8, 0.35))
cg.addPath(ribbon); cg.setFillColor(rgb(183, 131, 87)); cg.fillPath()
cg.restoreGState()
cg.saveGState()
cg.addPath(ribbon); cg.clip()
let sheen = CGGradient(colorsSpace: space, colors: [rgb(214, 163, 116), rgb(160, 108, 66)] as CFArray, locations: [0, 1])!
cg.drawLinearGradient(sheen, start: CGPoint(x: 640, y: 0), end: CGPoint(x: 700, y: 0), options: [])
cg.restoreGState()

NSGraphicsContext.restoreGraphicsState()
try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: out))
