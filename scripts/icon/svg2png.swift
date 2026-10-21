// Renders an SVG to a PNG with a transparent background: svg2png <in.svg> <out.png> <size> [pad]
import AppKit
let args = CommandLine.arguments
let size = Int(args[3])!
guard let img = NSImage(contentsOf: URL(fileURLWithPath: args[1])) else { fatalError("svg") }
let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: size, pixelsHigh: size, bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
NSColor.clear.set(); NSRect(x: 0, y: 0, width: size, height: size).fill()
let pad = args.count > 4 ? Double(size) * 100.0 / 1024.0 : 0; img.draw(in: NSRect(x: pad, y: pad, width: Double(size) - 2 * pad, height: Double(size) - 2 * pad))
NSGraphicsContext.restoreGraphicsState()
try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: args[2]))
