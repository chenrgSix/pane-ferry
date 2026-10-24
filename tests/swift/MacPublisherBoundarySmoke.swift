import AppKit
import Darwin
@main struct MacPublisherBoundarySmoke {
    static func main() throws {
        var sockets=[Int32](repeating:0,count:2)
        precondition(socketpair(AF_UNIX,SOCK_STREAM,0,&sockets)==0)
        let blocked=Wire(sockets[0])
        let started=ProcessInfo.processInfo.systemUptime
        precondition(blocked.send(["type":"frame","data":String(repeating:"a",count:2*1024*1024)]))
        // The peer intentionally never reads; a second control send and finish
        // must not wait for the socket's three-second write timeout.
        precondition(blocked.send(["type":"window.destroyed","window_id":1]))
        blocked.finish()
        precondition(ProcessInfo.processInfo.systemUptime-started<1)
        Darwin.close(sockets[1])
        let identity=ProcessIdentity.read(getpid())!.identity
        precondition(identity.current && identity.descends(from:identity))
        let reused=ProcessIdentity(pid:identity.pid,started:identity.started+1,executable:identity.executable)
        precondition(!reused.current && !reused.descends(from:identity) && !identity.descends(from:reused), "PID reuse cannot inherit membership")
        precondition(!identity.belongs(to:URL(fileURLWithPath:"/unrelated.app"),root:reused))
        precondition(receivedApplication(URL(fileURLWithPath:"/Applications/Firefox.app"),"io.lanremoteapp.host.fixture"))
        precondition(receivedApplication(URL(fileURLWithPath:"/Users/test/Library/Application Support/io.lanremoteapp.launcher/WindowHosts/generation/Firefox.app"),"org.mozilla.firefox"))
        precondition(receivedApplication(URL(fileURLWithPath:"/System/Library/CoreServices/SecurityAgent.app"),"com.apple.SecurityAgent"))
        precondition(!receivedApplication(URL(fileURLWithPath:"/Applications/Safari.app"),"com.apple.Safari"))
        precondition(dimensions(2560,1664));precondition(identifier("window-42"))
        for value in ["", "../Safari", "a;id",String(repeating:"a",count:101)] { precondition(!identifier(value)) }
        for (w,h) in [(0,1),(4096,4096),(4097,1),(-1,100)] { precondition(!dimensions(w,h)) }
        let cache=FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at:cache) }
        let pinned=UUID().uuidString, expired=UUID().uuidString
        for id in [pinned,expired] {
            try privateDirectory(cache.appendingPathComponent(id))
            try Data([1]).write(to:cache.appendingPathComponent(id).appendingPathComponent("file"))
        }
        try prepareClipboardCache(cache,incoming:1,protecting:[pinned],now:Date().addingTimeInterval(8*86400))
        precondition(FileManager.default.fileExists(atPath:cache.appendingPathComponent(pinned).path))
        precondition(!FileManager.default.fileExists(atPath:cache.appendingPathComponent(expired).path))
        var data=Data([137,80,78,71,13,10,26,10,0,0,0,13,73,72,68,82,0,0,0,10,0,0,0,10])
        try checkPNG(data);data[16]=127
        do {try checkPNG(data);fatalError("oversized image accepted")} catch NodeError.clipboard_invalid {}
        let frame=CGRect(x:-800,y:180,width:800,height:592),point=CGPoint(x:104,y:70)
        let event=windowMouseEvent(.leftMouseDown,windowID:42,frame:frame,location:point,flags:.maskShift,button:.left,number:17,pressure:1)!
        let native=NSEvent(cgEvent:event)!
        typealias GetWindowLocation=@convention(c)(CGEvent)->CGPoint
        let getLocation=unsafeBitCast(dlsym(UnsafeMutableRawPointer(bitPattern:-2),"CGEventGetWindowLocation")!,to:GetWindowLocation.self)
        precondition(native.windowNumber==42 && event.location==CGPoint(x:-696,y:250) && getLocation(event)==point)
        precondition(native.modifierFlags.contains(.shift) && native.eventNumber==17)
        var clicks=MouseClickSequence()
        for index in 1...3 {
            let down=clicks.event(button:1,pressed:true,point:.zero,now:Double(index)*0.1,interval:0.5)!
            let up=clicks.event(button:1,pressed:false,point:.zero,now:Double(index)*0.1+0.01,interval:0.5)!
            precondition(down.number==up.number && down.count==index && up.count==index)
        }
        precondition(clicks.event(button:1,pressed:false,point:.zero,now:1,interval:0.5)==nil)
        precondition(clicks.event(button:1,pressed:true,point:CGPoint(x:50,y:0),now:0.4,interval:0.5)!.count==1)
        let double=windowMouseEvent(.leftMouseDown,windowID:42,frame:frame,location:point,flags:[],button:.left,number:18,pressure:1,clickCount:2)!
        precondition(NSEvent(cgEvent:double)!.clickCount==2)
        let scrollEvent=windowScrollEvent(windowID:42,frame:frame,location:point,flags:[],vertical:3,horizontal:0)!
        let scroll=NSEvent(cgEvent:scrollEvent)!
        precondition(scroll.windowNumber==42 && scroll.scrollingDeltaY==3 && getLocation(scrollEvent)==point)
        let rate=VideoRateController()
        rate.configure(cap:90,feedback:true,now:0)
        precondition(rate.maximum()==90 && rate.desiredFPS(now:0)==90)
        let firstFrame=rate.sent(now:0.1)!
        precondition(!rate.acknowledge(firstFrame,now:0.15))
        let slowFrame=rate.sent(now:0.2)!
        precondition(!rate.acknowledge(slowFrame,now:0.5))
        let slowerFrame=rate.sent(now:0.55)!
        precondition(rate.acknowledge(slowerFrame,now:0.9) && rate.desiredFPS(now:0.9)==67)
        _=rate.sent(now:1.0)
        precondition(rate.desiredFPS(now:2.1)==33, "feedback stall must reduce capture rate")
        rate.configure(cap:60,feedback:false,now:3)
        precondition(rate.sent(now:3.1)==nil && rate.desiredFPS(now:3.1)==60)
        print("PASS: Mac publisher exclusion, identifiers, frame and image budgets")
    }
}
