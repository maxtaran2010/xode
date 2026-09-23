// Records one app window (no desktop, no shadow, no cursor) to PNG frames with alpha.
// Usage: swift scripts/record-window.swift <app name> <out dir> [fps=15] [max seconds=120]
// Stops when the window closes. Needs Screen Recording permission for the terminal.
import CoreImage
import CoreMedia
import Foundation
import ImageIO
import ScreenCaptureKit
import UniformTypeIdentifiers

let args = CommandLine.arguments
guard args.count >= 3 else {
    print("usage: record-window <app> <outdir> [fps] [maxsec]")
    exit(2)
}
let appName = args[1]
let outDir = URL(fileURLWithPath: args[2])
let fps = args.count > 3 ? Double(args[3]) ?? 15 : 15
let maxSec = args.count > 4 ? Double(args[4]) ?? 120 : 120
try? FileManager.default.createDirectory(at: outDir, withIntermediateDirectories: true)

final class Recorder: NSObject, SCStreamOutput, SCStreamDelegate {
    private let lock = NSLock()
    private var latest: CGImage?
    private let ctx = CIContext()
    var stopped = false

    func stream(_ stream: SCStream, didOutputSampleBuffer sb: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .screen, let pb = sb.imageBuffer else { return }
        if let arr = CMSampleBufferGetSampleAttachmentsArray(sb, createIfNecessary: false) as? [[SCStreamFrameInfo: Any]],
           let raw = arr.first?[.status] as? Int, raw != SCFrameStatus.complete.rawValue {
            return
        }
        let ci = CIImage(cvPixelBuffer: pb)
        guard let cg = ctx.createCGImage(ci, from: ci.extent, format: .BGRA8, colorSpace: CGColorSpace(name: CGColorSpace.sRGB)) else { return }
        lock.lock()
        latest = cg
        lock.unlock()
    }

    func stream(_ stream: SCStream, didStopWithError error: Error) {
        stopped = true
    }

    func current() -> CGImage? {
        lock.lock()
        defer { lock.unlock() }
        return latest
    }
}

func findWindow() async -> SCWindow? {
    guard let content = try? await SCShareableContent.excludingDesktopWindows(true, onScreenWindowsOnly: true) else { return nil }
    return content.windows.first { $0.owningApplication?.applicationName == appName && $0.frame.width >= 600 && $0.isOnScreen }
}

func save(_ img: CGImage, _ url: URL) {
    guard let d = CGImageDestinationCreateWithURL(url as CFURL, UTType.png.identifier as CFString, 1, nil) else { return }
    CGImageDestinationAddImage(d, img, nil)
    CGImageDestinationFinalize(d)
}

let rec = Recorder()
let sem = DispatchSemaphore(value: 0)
Task {
    do {
        _ = try await SCShareableContent.current
    } catch {
        print("screen recording not permitted: \(error.localizedDescription)")
        exit(3)
    }
    var win: SCWindow?
    for _ in 0..<600 {
        win = await findWindow()
        if win != nil { break }
        try? await Task.sleep(nanoseconds: 100_000_000)
    }
    guard var w = win else {
        print("window not found")
        exit(4)
    }
    // Let the window settle (resize/center happen right after launch).
    try? await Task.sleep(nanoseconds: 900_000_000)
    w = await findWindow() ?? w
    let filter = SCContentFilter(desktopIndependentWindow: w)
    let cfg = SCStreamConfiguration()
    let scale = Double(filter.pointPixelScale)
    cfg.width = Int(w.frame.width * scale)
    cfg.height = Int(w.frame.height * scale)
    cfg.minimumFrameInterval = CMTime(value: 1, timescale: CMTimeScale(fps * 2))
    cfg.showsCursor = false
    cfg.backgroundColor = CGColor.clear
    cfg.ignoreShadowsSingleWindow = true
    cfg.pixelFormat = kCVPixelFormatType_32BGRA
    cfg.queueDepth = 6
    let stream = SCStream(filter: filter, configuration: cfg, delegate: rec)
    try! stream.addStreamOutput(rec, type: .screen, sampleHandlerQueue: DispatchQueue(label: "rec"))
    try! await stream.startCapture()
    print("recording \(w.frame.width)x\(w.frame.height) @\(scale)x")
    let start = Date()
    var n = 0
    var lastCheck = Date()
    while !rec.stopped && Date().timeIntervalSince(start) < maxSec {
        let due = start.addingTimeInterval(Double(n) / fps)
        let wait = due.timeIntervalSinceNow
        if wait > 0 { try? await Task.sleep(nanoseconds: UInt64(wait * 1e9)) }
        if let img = rec.current() {
            save(img, outDir.appendingPathComponent(String(format: "f%05d.png", n)))
            n += 1
        } else {
            try? await Task.sleep(nanoseconds: 20_000_000)
        }
        if Date().timeIntervalSince(lastCheck) > 1 {
            lastCheck = Date()
            if await findWindow() == nil { break }
        }
    }
    try? await stream.stopCapture()
    print("frames \(n)")
    sem.signal()
}
sem.wait()
