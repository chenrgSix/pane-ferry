import AppKit
import ScreenCaptureKit
import ApplicationServices
import CryptoKit
import Darwin

struct PublishedApp {
    let id:String, name:String, url:URL, bundleID:String
    var publicValue:[String:Any] {
        ["id":id,"name":name,"description":"Mac 本机应用","source":"desktop","icon_png":icon(url) ?? NSNull()]
    }
}
func icon(_ url:URL) -> String? {
    let image=NSWorkspace.shared.icon(forFile:url.path)
    for size in [64,48,32] {
        guard let bitmap=NSBitmapImageRep(bitmapDataPlanes:nil,pixelsWide:size,pixelsHigh:size,bitsPerSample:8,samplesPerPixel:4,
            hasAlpha:true,isPlanar:false,colorSpaceName:.deviceRGB,bytesPerRow:0,bitsPerPixel:0) else { continue }
        NSGraphicsContext.saveGraphicsState();NSGraphicsContext.current=NSGraphicsContext(bitmapImageRep:bitmap)
        image.draw(in:NSRect(x:0,y:0,width:size,height:size));NSGraphicsContext.restoreGraphicsState()
        if let data=bitmap.representation(using:.png,properties:[:]),data.count<=12*1024 { return data.base64EncodedString() }
    }
    return nil
}
func receivedApplication(_ url:URL, _ bundleID:String) -> Bool {
    let path=url.resolvingSymlinksInPath().standardized.path
    return bundleID.hasPrefix("io.lanremoteapp.") || bundleID.hasPrefix("local.lanremoteapp.")
        || path.contains("/io.lanremoteapp.launcher/WindowHosts/")
        || bundleID == "com.apple.loginwindow" || bundleID == "com.apple.SecurityAgent"
}
func installedApps() -> [String:PublishedApp] {
    let roots=[URL(fileURLWithPath:"/Applications"),URL(fileURLWithPath:"/System/Applications"),
        URL(fileURLWithPath:"/System/Applications/Utilities"),FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Applications")]
    var urls=Set<URL>()
    for root in roots {
        guard let iterator=FileManager.default.enumerator(at:root,includingPropertiesForKeys:[.isDirectoryKey],options:[.skipsHiddenFiles,.skipsPackageDescendants]) else { continue }
        for case let url as URL in iterator where url.pathExtension == "app" { urls.insert(url) }
    }
    for app in NSWorkspace.shared.runningApplications { if let url=app.bundleURL { urls.insert(url) } }
    var result=[String:PublishedApp]()
    for url in urls.sorted(by:{$0.path<$1.path}) {
        guard let bundle=Bundle(url:url),let id=bundle.bundleIdentifier,!receivedApplication(url,id),
              bundle.object(forInfoDictionaryKey:"LSUIElement") as? Bool != true,
              bundle.object(forInfoDictionaryKey:"LSBackgroundOnly") as? Bool != true else { continue }
        let hash=SHA256.hash(data:Data((id+"\n"+url.resolvingSymlinksInPath().path).utf8)).prefix(16).map{String(format:"%02x",$0)}.joined()
        let key="desktop-\(hash)"
        let name=(bundle.object(forInfoDictionaryKey:"CFBundleDisplayName") as? String)
            ?? (bundle.object(forInfoDictionaryKey:"CFBundleName") as? String) ?? url.deletingPathExtension().lastPathComponent
        result[key]=PublishedApp(id:key,name:String(name.prefix(256)),url:url,bundleID:id)
        if result.count>=512 { break }
    }
    return result
}
struct Target {
    let window:SCWindow, application:NSRunningApplication, app:PublishedApp
    let launched:Date
    var id:String { "window-\(window.windowID)-\(application.processIdentifier)-\(Int64(launched.timeIntervalSince1970*1000))" }
}
// PID delivery needs both screen and window-local coordinates. AppKit's factory
// supplies the receiving-window header; CGEventSetLocation alone overwrites the
// hit-test location. Resolve the window-local setter at runtime and fail closed
// if this OS does not support it. Events are never posted to a global event tap.
private typealias SetWindowLocation=@convention(c)(CGEvent,CGPoint)->Void
private let setWindowLocation:SetWindowLocation? = {
    guard let symbol=dlsym(UnsafeMutableRawPointer(bitPattern:-2),"CGEventSetWindowLocation") else { return nil }
    return unsafeBitCast(symbol,to:SetWindowLocation.self)
}()
func targetPointerEvent(_ event:CGEvent,windowID:UInt32,frame:CGRect,location:CGPoint) -> CGEvent? {
    guard let setLocation=setWindowLocation else { return nil }
    event.setIntegerValueField(.mouseEventWindowUnderMousePointer,value:Int64(windowID))
    event.setIntegerValueField(.mouseEventWindowUnderMousePointerThatCanHandleThisEvent,value:Int64(windowID))
    event.location=CGPoint(x:frame.minX+location.x,y:frame.minY+location.y)
    setLocation(event,location)
    return event
}
func windowMouseEvent(_ type:NSEvent.EventType,windowID:UInt32,frame:CGRect,location:CGPoint,flags:CGEventFlags,button:CGMouseButton,number:Int,pressure:Float) -> CGEvent? {
    let base=CGPoint(x:location.x,y:frame.height-location.y)
    guard let native=NSEvent.mouseEvent(with:type,location:base,modifierFlags:NSEvent.ModifierFlags(rawValue:UInt(flags.rawValue)),
        timestamp:ProcessInfo.processInfo.systemUptime,windowNumber:Int(windowID),context:nil,eventNumber:number,clickCount:1,pressure:pressure),
        let event=native.cgEvent else { return nil }
    event.setIntegerValueField(.mouseEventButtonNumber,value:Int64(button.rawValue))
    event.setIntegerValueField(.mouseEventSubtype,value:3)
    return targetPointerEvent(event,windowID:windowID,frame:frame,location:location)
}
func windowScrollEvent(windowID:UInt32,frame:CGRect,location:CGPoint,flags:CGEventFlags,vertical:Int32,horizontal:Int32) -> CGEvent? {
    guard let event=CGEvent(scrollWheelEvent2Source:nil,units:.line,wheelCount:2,wheel1:vertical,wheel2:horizontal,wheel3:0) else { return nil }
    // Scroll has no NSEvent factory; this is the receiving-window header filled
    // by that factory for mouse events, separate from the hit-test window tags.
    guard let windowField=CGEventField(rawValue:51) else { return nil }
    event.setIntegerValueField(windowField,value:Int64(windowID));event.flags=flags
    return targetPointerEvent(event,windowID:windowID,frame:frame,location:location)
}
final class PublishedSession {
    let id:String, appID:String, target:Target
    var controller:UUID?, revision:UInt64=1
    var width:Int, height:Int
    var keys=Set<String>(), buttons=Set<Int>()
    var pointerSerial=0
    var lastFrame:CGRect
    init(_ target:Target,appID:String) {
        id=UUID().uuidString.replacingOccurrences(of:"-",with:"").lowercased();self.target=target;self.appID=appID
        lastFrame=target.window.frame
        let scale=min(2.0,sqrt(Double(pixelLimit)/max(1,lastFrame.width*lastFrame.height)))
        width=min(4096,max(1,Int(lastFrame.width*scale)));height=min(4096,max(1,Int(lastFrame.height*scale)))
    }
    var metadata:[String:Any] {
        ["type":"window.created","window_id":target.window.windowID,"parent_id":NSNull(),"title":String((target.window.title ?? target.app.name).prefix(512)),
         "role":"toplevel","x":0,"y":0,"width":width,"height":height,"scale_factor":2,"geometry_revision":revision,"visible":true]
    }
    func valid() -> Bool {
        guard !target.application.isTerminated, target.application.launchDate == target.launched,
              !receivedApplication(target.application.bundleURL ?? target.app.url,target.application.bundleIdentifier ?? "") else { return false }
        guard let list=CGWindowListCopyWindowInfo([.optionIncludingWindow,.excludeDesktopElements],target.window.windowID) as? [[String:Any]],
            let value=list.first, value[kCGWindowOwnerPID as String] as? Int32 == target.application.processIdentifier,
            let bounds=value[kCGWindowBounds as String] as? [String:Any],
            let frame=CGRect(dictionaryRepresentation:bounds as CFDictionary) else { return false }
        lastFrame=frame
        return true
    }
    func axWindow() throws -> AXUIElement {
        guard valid(), AXIsProcessTrusted() else { throw NodeError.accessibility_required }
        let app=AXUIElementCreateApplication(target.application.processIdentifier)
        var values:CFTypeRef?
        guard AXUIElementCopyAttributeValue(app,kAXWindowsAttribute as CFString,&values) == .success,
              let windows=values as? [AXUIElement] else { throw NodeError.window_unavailable }
        var matches=[AXUIElement]()
        for window in windows {
            var p:CFTypeRef?,s:CFTypeRef?
            guard AXUIElementCopyAttributeValue(window,kAXPositionAttribute as CFString,&p) == .success,
                  AXUIElementCopyAttributeValue(window,kAXSizeAttribute as CFString,&s) == .success,
                  let p=p,let s=s,CFGetTypeID(p)==AXValueGetTypeID(),CFGetTypeID(s)==AXValueGetTypeID() else { continue }
            var point=CGPoint.zero,size=CGSize.zero
            AXValueGetValue(p as! AXValue,.cgPoint,&point);AXValueGetValue(s as! AXValue,.cgSize,&size)
            if abs(point.x-lastFrame.minX)<3 && abs(point.y-lastFrame.minY)<3
                && abs(size.width-lastFrame.width)<3 && abs(size.height-lastFrame.height)<3 { matches.append(window) }
        }
        guard matches.count == 1 else { throw NodeError.window_unavailable }; return matches[0]
    }
    func focus() throws {
        let window=try axWindow()
        target.application.activate(options:[])
        let app=AXUIElementCreateApplication(target.application.processIdentifier)
        AXUIElementSetAttributeValue(app,kAXFocusedWindowAttribute as CFString,window)
        guard AXUIElementPerformAction(window,kAXRaiseAction as CFString) == .success else { throw NodeError.window_unavailable }
    }
    func release() {
        for key in keys { try? keyEvent(key,false) }; keys.removeAll()
        for button in buttons { try? pointer(0,0,button,false) }; buttons.removeAll()
    }
    func ensureInput() throws {
        guard valid() else { throw NodeError.session_exited }
        guard AXIsProcessTrusted() else { throw NodeError.accessibility_required }
        guard !desktopLocked() else { throw NodeError.desktop_locked }
    }
    func pointer(_ x:Int,_ y:Int,_ button:Int,_ pressed:Bool) throws {
        if pressed || button == 0 { try ensureInput() }
        else { guard valid() else { throw NodeError.session_exited } }
        guard x>=0,y>=0,x<width,y<height,(0...7).contains(button) else { throw NodeError.invalid_request }
        let point=CGPoint(x:Double(x)*lastFrame.width/Double(width),y:Double(y)*lastFrame.height/Double(height))
        pointerSerial=pointerSerial%Int(Int32.max)+1
        if (4...7).contains(button) {
            if pressed {
                guard let moved=windowMouseEvent(.mouseMoved,windowID:target.window.windowID,frame:lastFrame,location:point,flags:flags,button:.left,number:pointerSerial,pressure:0),
                    let event=windowScrollEvent(windowID:target.window.windowID,frame:lastFrame,location:point,flags:flags,vertical:button==4 ? 3 : button==5 ? -3 : 0,horizontal:button==6 ? 3 : button==7 ? -3 : 0) else { throw NodeError.input_unsupported }
                moved.postToPid(target.application.processIdentifier)
                event.postToPid(target.application.processIdentifier)
            };return
        }
        let active=button==0 ? (buttons.contains(1) ? 1 : buttons.contains(3) ? 3 : buttons.contains(2) ? 2 : 0) : button
        let mouse:CGMouseButton = active==3 ? .right : active==2 ? .center : .left
        let type:NSEvent.EventType = button==0 ? (active==1 ? .leftMouseDragged : active==3 ? .rightMouseDragged : active==2 ? .otherMouseDragged : .mouseMoved)
            : button==1 ? (pressed ? .leftMouseDown:.leftMouseUp) : button==3 ? (pressed ? .rightMouseDown:.rightMouseUp) : (pressed ? .otherMouseDown:.otherMouseUp)
        guard let event=windowMouseEvent(type,windowID:target.window.windowID,frame:lastFrame,location:point,flags:flags,button:mouse,number:pointerSerial,pressure:pressed || (button==0 && active != 0) ? 1:0) else { throw NodeError.input_unsupported }
        event.postToPid(target.application.processIdentifier)
        if button>0 { if pressed { buttons.insert(button) } else { buttons.remove(button) } }
    }
    var flags:CGEventFlags {
        var result=CGEventFlags()
        if keys.contains("Control_L") || keys.contains("Control_R") { result.insert(.maskCommand) }
        if keys.contains("Super_L") || keys.contains("Super_R") { result.insert(.maskControl) }
        if keys.contains("Alt_L") || keys.contains("Alt_R") { result.insert(.maskAlternate) }
        if keys.contains("Shift_L") || keys.contains("Shift_R") { result.insert(.maskShift) }
        return result
    }
    func keyEvent(_ key:String,_ pressed:Bool) throws {
        if pressed { try ensureInput() } else { guard valid() else { throw NodeError.session_exited } }
        let map:[String:CGKeyCode] = ["a":0,"s":1,"d":2,"f":3,"h":4,"g":5,"z":6,"x":7,"c":8,"v":9,"b":11,"q":12,"w":13,"e":14,"r":15,"y":16,"t":17,
            "1":18,"2":19,"3":20,"4":21,"6":22,"5":23,"equal":24,"9":25,"7":26,"minus":27,"8":28,"0":29,"bracketright":30,"o":31,"u":32,"bracketleft":33,"i":34,"p":35,
            "Return":36,"l":37,"j":38,"apostrophe":39,"k":40,"semicolon":41,"backslash":42,"comma":43,"slash":44,"n":45,"m":46,"period":47,"Tab":48,"space":49,"grave":50,
            "BackSpace":51,"Escape":53,"Super_L":59,"Super_R":62,"Control_L":55,"Control_R":54,"Shift_L":56,"Shift_R":60,"Alt_L":58,"Alt_R":61,
            "Left":123,"Right":124,"Down":125,"Up":126,"Home":115,"End":119,"Page_Up":116,"Page_Down":121,"Delete":117,"F1":122,"F2":120,"F3":99,"F4":118,"F5":96,"F6":97,"F7":98,"F8":100,"F9":101,"F10":109,"F11":103,"F12":111]
        guard let code=map[key.lowercased()] ?? map[key] else { throw NodeError.input_unsupported }
        if pressed { keys.insert(key) } else { keys.remove(key) }
        guard let event=CGEvent(keyboardEventSource:nil,virtualKey:code,keyDown:pressed) else { throw NodeError.input_unsupported }
        event.flags=flags;event.postToPid(target.application.processIdentifier)
    }
    func text(_ text:String) throws {
        try ensureInput();guard !text.isEmpty,text.utf8.count<=8192,!text.contains("\0") else { throw NodeError.invalid_request }
        let utf16=Array(text.utf16)
        var start=0
        while start<utf16.count {
            var end=min(start+16,utf16.count)
            if end<utf16.count && (0xD800...0xDBFF).contains(utf16[end-1]) { end-=1 }
            let part=Array(utf16[start..<end]);start=end
            for down in [true,false] {
                guard let event=CGEvent(keyboardEventSource:nil,virtualKey:0,keyDown:down) else { throw NodeError.input_unsupported }
                event.flags=[];part.withUnsafeBufferPointer { event.keyboardSetUnicodeString(stringLength:part.count,unicodeString:$0.baseAddress!) }
                event.postToPid(target.application.processIdentifier)
            }
        }
    }
    func shortcut(_ code:CGKeyCode) throws {
        try ensureInput();try focus()
        for down in [true,false] { let event=CGEvent(keyboardEventSource:nil,virtualKey:code,keyDown:down)!;event.flags = .maskCommand;event.postToPid(target.application.processIdentifier) }
    }
}
func desktopLocked() -> Bool {
    let session=CGSessionCopyCurrentDictionary() as? [String:Any]
    return session?["CGSSessionScreenIsLocked"] as? Bool == true || session?["kCGSessionOnConsoleKey"] as? Bool == false
}

final class Publisher {
    let directory:URL, deviceID:String
    var sharing=false, apps=[String:PublishedApp](), targets=[String:Target](), sessions=[String:PublishedSession](), requests=[String:String]()
    init(directory:URL) throws {
        self.directory=directory;try privateDirectory(directory)
        let path=directory.appendingPathComponent("device-id")
        if let value=try? String(contentsOf:path,encoding:.utf8),UUID(uuidString:value) != nil { deviceID=value }
        else { let value=UUID().uuidString.lowercased();try value.write(to:path,atomically:true,encoding:.utf8);chmod(path.path,0o600);deviceID=value }
        sharing=(try? Data(contentsOf:directory.appendingPathComponent("sharing.json"))).flatMap{try? JSONSerialization.jsonObject(with:$0) as? [String:Bool]}?["enabled"] ?? false
    }
    func info() -> [String:Any] {
        ["type":"node.info","device_id":deviceID,"name":Host.current().localizedName ?? "Mac","platform":"macos",
         "publishing_supported":true,"receiving_supported":true,"sharing_enabled":sharing,
         "permissions":["screen_recording":CGPreflightScreenCaptureAccess(),"accessibility":AXIsProcessTrusted(),"desktop_unlocked":!desktopLocked()],
         "session_count":sessions.count]
    }
    func scan() async throws -> [Target] {
        guard sharing else { throw NodeError.sharing_disabled }
        guard CGPreflightScreenCaptureAccess() else { throw NodeError.screen_recording_required }
        let content=try await SCShareableContent.excludingDesktopWindows(true,onScreenWindowsOnly:false)
        let current=apps.isEmpty ? installedApps() : apps
        var result=[Target]()
        for window in content.windows where window.windowLayer == 0 && window.frame.width>=100 && window.frame.height>=60 {
            guard let owner=window.owningApplication,let app=NSRunningApplication(processIdentifier:owner.processID),
                  app.activationPolicy == .regular,let url=app.bundleURL,let id=app.bundleIdentifier,
                  !receivedApplication(url,id),let launched=app.launchDate,
                  let entry=current.values.first(where:{$0.url.resolvingSymlinksInPath()==url.resolvingSymlinksInPath()}) else { continue }
            result.append(Target(window:window,application:app,app:entry,launched:launched))
            if result.count>=128 { break }
        }
        targets=Dictionary(uniqueKeysWithValues:result.map{($0.id,$0)});return result
    }
    func launch(_ appID:String, _ request:String) async throws -> PublishedSession {
        guard sharing else { throw NodeError.sharing_disabled }
        guard identifier(appID),identifier(request) else { throw NodeError.invalid_request }
        if let id=requests[request] {
            guard let session=sessions[id],session.valid() else { throw NodeError.session_exited }
            guard session.appID==appID else { throw NodeError.request_id_conflict };return session
        }
        sessions=sessions.filter{$0.value.valid()}
        guard sessions.count<16, requests.count<4096 else { throw NodeError.session_limit }
        var target=targets[appID]
        if target == nil {
            if apps[appID]==nil { apps=installedApps() }
            guard let app=apps[appID],!receivedApplication(app.url,app.bundleID),Bundle(url:app.url)?.bundleIdentifier==app.bundleID else { throw NodeError.app_not_approved }
            let config=NSWorkspace.OpenConfiguration();config.activates=false
            _ = try await NSWorkspace.shared.openApplication(at:app.url,configuration:config)
            for _ in 0..<40 {
                let windows=try await scan()
                if let found=windows.first(where:{$0.app.id==appID}) { target=found;break }
                try await Task.sleep(nanoseconds:100_000_000)
            }
        }
        guard let target=target else { throw NodeError.window_unavailable }
        if let session=sessions.values.first(where:{$0.target.id==target.id && $0.valid()}) { requests[request]=session.id;return session }
        let session=PublishedSession(target,appID:appID);sessions[session.id]=session;requests[request]=session.id;return session
    }
}
