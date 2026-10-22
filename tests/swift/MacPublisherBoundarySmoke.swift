import AppKit
import Darwin
@main struct MacPublisherBoundarySmoke {
    static func main() throws {
        let first=WindowInteraction(session:"a",window:1,at:100),second=WindowInteraction(session:"b",window:2,at:101)
        precondition(uniqueRecentInteraction([first],now:102)?.window==1)
        precondition(uniqueRecentInteraction([first,second],now:102)==nil, "ambiguous multi-device input must not guess popup ownership")
        precondition(uniqueRecentInteraction([first,second],now:105)?.window==2)
        precondition(uniqueRecentInteraction([second],now:99)==nil)
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
        let scrollEvent=windowScrollEvent(windowID:42,frame:frame,location:point,flags:[],vertical:3,horizontal:0)!
        let scroll=NSEvent(cgEvent:scrollEvent)!
        precondition(scroll.windowNumber==42 && scroll.scrollingDeltaY==3 && getLocation(scrollEvent)==point)
        print("PASS: Mac publisher exclusion, identifiers, frame and image budgets")
    }
}
