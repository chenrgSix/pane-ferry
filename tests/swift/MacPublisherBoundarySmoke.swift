import AppKit
@main struct MacPublisherBoundarySmoke {
    static func main() throws {
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
        let event=windowMouseEvent(.leftMouseDown,windowID:42,location:CGPoint(x:104,y:346),flags:.maskShift,button:.left,number:17,pressure:1)!
        let native=NSEvent(cgEvent:event)!
        precondition(native.windowNumber==42 && native.locationInWindow==CGPoint(x:104,y:346))
        precondition(native.modifierFlags.contains(.shift) && native.eventNumber==17)
        event.type = .scrollWheel;event.setIntegerValueField(.scrollWheelEventDeltaAxis1,value:3)
        let scroll=NSEvent(cgEvent:event)!
        precondition(scroll.windowNumber==42 && scroll.locationInWindow==CGPoint(x:104,y:346) && scroll.scrollingDeltaY==3)
        print("PASS: Mac publisher exclusion, identifiers, frame and image budgets")
    }
}
