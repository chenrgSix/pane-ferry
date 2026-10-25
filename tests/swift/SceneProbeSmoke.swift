import AppKit
import AVFoundation
@main struct SceneProbeSmoke {
    static func image(_ patch: CGRect? = nil) -> CGImage {
        let context = CGContext(data:nil,width:200,height:200,bitsPerComponent:8,bytesPerRow:800,
            space:CGColorSpaceCreateDeviceRGB(),bitmapInfo:CGImageAlphaInfo.premultipliedLast.rawValue)!
        context.setFillColor(CGColor(gray:0.1,alpha:1));context.fill(CGRect(x:0,y:0,width:200,height:200))
        if let patch { context.setFillColor(CGColor(gray:0.8,alpha:1));context.fill(patch) }
        return context.makeImage()!
    }
    static func main() throws {
        _ = NSApplication.shared
        let options = try Options.parse(["--native-peer","aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa","--native-state","/tmp/unused","--native-helper","/usr/bin/false","--session","fixture"])
        let app = AppDelegate(options:options,clipboard:NSPasteboard.withUniqueName())
        let host = HostedWindow(id:1,width:200,height:200,revision:1,scale:1,popup:false,disconnect:{}) {_ in}
        host.sessionMode=true;app.windows[1]=host;Probe.delegate=app;Probe.rootID=1
        Probe.menuRect=CGRect(x:50,y:50,width:100,height:100)
        let closed=image(), menu=image(CGRect(x:50,y:50,width:100,height:100))
        // An asymmetric image catches top/bottom inversion in the direct
        // NV12/BGRA ROI path, independently of menu matching state.
        var buffer: CVPixelBuffer?
        precondition(CVPixelBufferCreate(kCFAllocatorDefault, 200, 200, kCVPixelFormatType_32BGRA,
            [kCVPixelBufferIOSurfacePropertiesKey: [:]] as CFDictionary, &buffer) == kCVReturnSuccess)
        CVPixelBufferLockBaseAddress(buffer!, [])
        let context = CGContext(data: CVPixelBufferGetBaseAddress(buffer!), width: 200, height: 200,
            bitsPerComponent: 8, bytesPerRow: CVPixelBufferGetBytesPerRow(buffer!), space: CGColorSpaceCreateDeviceRGB(),
            bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue)!
        context.draw(image(CGRect(x: 0, y: 0, width: 200, height: 100)), in: CGRect(x: 0, y: 0, width: 200, height: 200))
        CVPixelBufferUnlockBaseAddress(buffer!, [])
        let video = try VideoFrame(buffer: buffer!, width: 200, height: 200)
        let savedROI = Probe.menuRect
        Probe.menuRect = CGRect(x: 17, y: 10, width: 51, height: 20)
        precondition(Probe.distance(Probe.signature(video)!, Probe.signature(video.image!)!) < 1,
                     "direct ROI must sample the same top-left pixels as the full image")
        Probe.menuRect = savedROI
        Probe.baseline=Probe.signature(closed);Probe.menuReference=Probe.signature(menu)
        Probe.phase="opening";Probe.phaseStarted=Probe.now();Probe.preview=false
        Probe.current=["start":Probe.phaseStarted,"warmup":false]
        func draw(_ image:CGImage, stale:Bool=false, id:UInt32=1) {
            Probe.frames[ObjectIdentifier(image)]=["window_id":id,"received":stale ? 0 : Probe.now()]
            Probe.drew(image)
        }
        draw(closed);precondition(Probe.rows.isEmpty)
        draw(image(CGRect(x:0,y:0,width:30,height:30)));precondition(Probe.rows.isEmpty,"outside-ROI changes are not menu latency")
        draw(image(CGRect(x:50,y:50,width:20,height:100)));precondition(Probe.rows.isEmpty,"partial changes do not match the menu preview")
        draw(menu,stale:true);precondition(Probe.rows.isEmpty)
        draw(menu,id:2);precondition(Probe.rows.isEmpty)
        draw(menu);precondition(Probe.rows.count==1 && Probe.phase=="drawn")
        Probe.phase="closing";Probe.phaseStarted=Probe.now();Probe.sceneClosed=false
        draw(menu);precondition(!Probe.sceneClosed)
        draw(closed);precondition(Probe.sceneClosed,"next sample waits for the closed baseline")
        print("PASS: scene latency ROI excludes unrelated/partial/stale/other-window changes, matches inspected menu and waits for close")
    }
}
