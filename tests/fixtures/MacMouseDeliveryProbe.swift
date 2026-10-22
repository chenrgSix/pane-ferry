// Opt-in native diagnostic. Never targets user applications or a global tap.
// Build with the publisher's Wire.swift, Publisher.swift and Clipboard.swift.
import AppKit
import ApplicationServices

private let fixtureURL=URL(fileURLWithPath:"/private/tmp/LRAMacPublishingTest.app")
private let stateURL=URL(fileURLWithPath:"/private/tmp/lra-mac-publishing-live-state.json")
private let resultURL=URL(fileURLWithPath:"/private/tmp/lra-mouse-delivery-probe.json")
private struct Recipe {
    let name:String
    var raw=false,stable=true,source:CGEventSourceStateID?=nil,subtype:Int64=3,command=false,targetPID=false
    var windowLocation=0 // 0 top-down, 1 bottom-up, 2 no window-local setter
    var localDelivery=false,zeroNumber=false
}
@MainActor final class MouseProbe:NSObject,NSApplicationDelegate {
    var window:NSWindow!,status:NSTextField!,start:NSButton!,busy=false,stage="idle"
    func applicationDidFinishLaunching(_ notification:Notification) {
        let menu=NSMenu(),item=NSMenuItem(),actions=NSMenu()
        actions.addItem(withTitle:"Quit",action:#selector(NSApplication.terminate(_:)),keyEquivalent:"q")
        item.submenu=actions;menu.addItem(item);NSApplication.shared.mainMenu=menu
        window=NSWindow(contentRect:NSRect(x:240,y:240,width:620,height:180),styleMask:[.titled,.closable],backing:.buffered,defer:false)
        window.title="窗渡 PaneFerry 鼠标诊断（仅专用测试窗口）"
        status=NSTextField(wrappingLabelWithString:"只操作 PaneFerry Mac Test，不读取其他应用、不联网、不注入全桌面事件。")
        status.frame=NSRect(x:20,y:90,width:580,height:65);window.contentView?.addSubview(status)
        let permission=NSButton(title:"请求诊断辅助功能权限",target:self,action:#selector(requestPermission))
        permission.frame=NSRect(x:20,y:25,width:240,height:40);window.contentView?.addSubview(permission)
        start=NSButton(title:"开始诊断",target:self,action:#selector(run));start.frame=NSRect(x:300,y:25,width:180,height:40);window.contentView?.addSubview(start)
        window.makeKeyAndOrderFront(nil);NSApplication.shared.activate(ignoringOtherApps:true)
    }
    @objc func requestPermission() {
        _=AXIsProcessTrustedWithOptions([kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String:true] as CFDictionary)
    }
    func state() throws -> [String:Any] {
        guard let body=try JSONSerialization.jsonObject(with:Data(contentsOf:stateURL)) as? [String:Any] else {throw NodeError.invalid_request};return body
    }
    func target() throws -> (NSRunningApplication,UInt32,CGRect,AXUIElement) {
        stage="fixture_identity"
        let body=try state()
        guard let pid=body["pid"] as? Int32,let wid=body["window_number"] as? UInt32,
            let app=NSRunningApplication(processIdentifier:pid),!app.isTerminated,app.launchDate != nil,
            app.bundleIdentifier=="dev.lra.MacPublishingFixture",app.bundleURL?.resolvingSymlinksInPath().path==fixtureURL.resolvingSymlinksInPath().path else {throw NodeError.window_unavailable}
        // This diagnostic has only Accessibility permission. Get geometry from
        // that API rather than requiring a second Screen Recording grant.
        stage="fixture_accessibility"
        var value:CFTypeRef?
        guard AXUIElementCopyAttributeValue(AXUIElementCreateApplication(pid),kAXWindowsAttribute as CFString,&value) == .success,
            let windows=value as? [AXUIElement] else {throw NodeError.window_unavailable}
        let matches=windows.filter { window in
            var title:CFTypeRef?
            return AXUIElementCopyAttributeValue(window,kAXTitleAttribute as CFString,&title) == .success && title as? String=="窗渡 PaneFerry Mac 发布测试"
        }
        guard matches.count==1 else {throw NodeError.window_unavailable}
        stage="fixture_geometry"
        var position:CFTypeRef?,size:CFTypeRef?
        guard AXUIElementCopyAttributeValue(matches[0],kAXPositionAttribute as CFString,&position) == .success,
            AXUIElementCopyAttributeValue(matches[0],kAXSizeAttribute as CFString,&size) == .success,
            let position=position,let size=size,CFGetTypeID(position)==AXValueGetTypeID(),CFGetTypeID(size)==AXValueGetTypeID() else {throw NodeError.window_unavailable}
        var point=CGPoint.zero,dimensions=CGSize.zero
        AXValueGetValue(position as! AXValue,.cgPoint,&point);AXValueGetValue(size as! AXValue,.cgSize,&dimensions)
        guard dimensions.width>200,dimensions.height>150 else {throw NodeError.window_unavailable}
        return (app,wid,CGRect(origin:point,size:dimensions),matches[0])
    }
    private func make(_ recipe:Recipe,_ type:NSEvent.EventType,_ app:NSRunningApplication,_ wid:UInt32,_ frame:CGRect,_ number:Int) -> CGEvent? {
        let point=CGPoint(x:104,y:74),global=CGPoint(x:frame.minX+104,y:frame.minY+74)
        let flags:CGEventFlags=recipe.command ? .maskCommand:[]
        if recipe.localDelivery {
            let serial=recipe.zeroNumber ? 0:number
            if recipe.raw {
                guard let event=CGEvent(mouseEventSource:nil,mouseType:type == .leftMouseDown ? .leftMouseDown:.leftMouseUp,mouseCursorPosition:point,mouseButton:.left) else {return nil}
                event.setIntegerValueField(CGEventField(rawValue:51)!,value:Int64(wid))
                event.setIntegerValueField(.mouseEventNumber,value:Int64(serial));event.setIntegerValueField(.mouseEventClickState,value:1)
                event.setIntegerValueField(.mouseEventWindowUnderMousePointer,value:Int64(wid))
                event.setIntegerValueField(.mouseEventWindowUnderMousePointerThatCanHandleThisEvent,value:Int64(wid))
                event.flags=flags;return event
            }
            // The posting process has no NSWindow for this number. The native
            // factory flips against its screen, not the recipient's frame.
            let height=NSScreen.screens.first?.frame.height ?? 0
            return NSEvent.mouseEvent(with:type,location:CGPoint(x:point.x,y:height-point.y),modifierFlags:NSEvent.ModifierFlags(rawValue:UInt(flags.rawValue)),timestamp:ProcessInfo.processInfo.systemUptime,windowNumber:Int(wid),context:nil,eventNumber:serial,clickCount:1,pressure:type == .leftMouseDown ? 1:0)?.cgEvent
        }
        let event:CGEvent?
        if recipe.raw {
            event=CGEvent(mouseEventSource:recipe.source.flatMap{CGEventSource(stateID:$0)},mouseType:type == .leftMouseDown ? .leftMouseDown:.leftMouseUp,mouseCursorPosition:global,mouseButton:.left)
            event?.setIntegerValueField(CGEventField(rawValue:51)!,value:Int64(wid))
            event?.setIntegerValueField(.mouseEventNumber,value:Int64(number))
            event?.setIntegerValueField(.mouseEventClickState,value:1)
            event?.setDoubleValueField(.mouseEventPressure,value:type == .leftMouseDown ? 1:0)
        } else {
            event=windowMouseEvent(type,windowID:wid,frame:frame,location:point,flags:flags,button:.left,number:number,pressure:type == .leftMouseDown ? 1:0)
            if let source=recipe.source {event?.setSource(CGEventSource(stateID:source))}
        }
        guard let event=event else {return nil}
        event.flags=flags;event.setIntegerValueField(.mouseEventSubtype,value:recipe.subtype)
        if recipe.targetPID {event.setIntegerValueField(.eventTargetUnixProcessID,value:Int64(app.processIdentifier))}
        if recipe.windowLocation==0 {return targetPointerEvent(event,windowID:wid,frame:frame,location:point)}
        if recipe.windowLocation==1 {
            _=targetPointerEvent(event,windowID:wid,frame:frame,location:CGPoint(x:104,y:frame.height-74))
            event.location=global
            return event
        }
        event.location=global
        event.setIntegerValueField(.mouseEventWindowUnderMousePointer,value:Int64(wid))
        event.setIntegerValueField(.mouseEventWindowUnderMousePointerThatCanHandleThisEvent,value:Int64(wid))
        return event
    }
    @objc func run() {
        guard !busy else {return}
        let trusted=AXIsProcessTrusted(),postAccess=CGPreflightPostEventAccess()
        guard trusted,postAccess else {
            let info:[String:Any]=["accessibility":trusted,"post_event_access":postAccess,"results":[]]
            try? JSONSerialization.data(withJSONObject:info).write(to:resultURL,options:.atomic)
            status.stringValue="需要诊断程序本身的辅助功能权限；尚未发送输入。";return
        }
        busy=true;start.isEnabled=false
        Task { @MainActor in
            var results=[[String:Any]]()
            let recipes=[Recipe(name:"current-unstable",stable:false),Recipe(name:"native-stable"),
                Recipe(name:"native-stable-hid",source:.hidSystemState),Recipe(name:"native-stable-private",source:.privateState),
                Recipe(name:"native-stable-subtype0",subtype:0),Recipe(name:"native-stable-command",command:true),
                Recipe(name:"native-stable-pid",targetPID:true),
                Recipe(name:"native-bottom-up",windowLocation:1),Recipe(name:"native-screen-only",windowLocation:2),
                Recipe(name:"raw-stable",raw:true,subtype:0),Recipe(name:"raw-stable-subtype3",raw:true),
                Recipe(name:"raw-stable-hid",raw:true,source:.hidSystemState,subtype:0),
                Recipe(name:"raw-stable-private",raw:true,source:.privateState,subtype:0),
                Recipe(name:"raw-stable-pid",raw:true,subtype:0,targetPID:true),
                Recipe(name:"raw-stable-command",raw:true,subtype:0,command:true),
                Recipe(name:"factory-local-stable",localDelivery:true),
                Recipe(name:"factory-local-zero",localDelivery:true,zeroNumber:true),
                Recipe(name:"factory-local-command",command:true,localDelivery:true),
                Recipe(name:"raw-local-stable",raw:true,localDelivery:true),
                Recipe(name:"raw-local-zero",raw:true,localDelivery:true,zeroNumber:true)]
            do {
                for (index,recipe) in recipes.enumerated() {
                    status.stringValue="测试 \(index+1)/\(recipes.count)：\(recipe.name)"
                    let (app,wid,frame,ax)=try target()
                    guard AXUIElementPerformAction(ax,kAXRaiseAction as CFString) == .success else {throw NodeError.window_unavailable}
                    stage="fixture_activation"
                    let axApp=AXUIElementCreateApplication(app.processIdentifier)
                    AXUIElementSetAttributeValue(axApp,kAXFrontmostAttribute as CFString,kCFBooleanTrue)
                    AXUIElementSetAttributeValue(axApp,kAXFocusedWindowAttribute as CFString,ax)
                    app.activate(options:[])
                    try await Task.sleep(nanoseconds:200_000_000)
                    guard app.isActive else {throw NodeError.window_unavailable}
                    let before=try state()["clicks"] as! Int
                    for down in [true,false] {
                        stage="fixture_input"
                        let live=try state()
                        guard !app.isTerminated,live["pid"] as? Int32==app.processIdentifier,live["window_number"] as? UInt32==wid else {throw NodeError.window_unavailable}
                        let number=1000+index*2+(!down && !recipe.stable ? 1:0)
                        guard let event=make(recipe,down ? .leftMouseDown:.leftMouseUp,app,wid,frame,number) else {throw NodeError.input_unsupported}
                        event.postToPid(app.processIdentifier)
                        try await Task.sleep(nanoseconds:80_000_000)
                    }
                    try await Task.sleep(nanoseconds:200_000_000)
                    let after=try state()["clicks"] as! Int
                    results.append(["recipe":recipe.name,"click_delta":after-before,"events":try state()["input_events"] ?? []])
                }
                status.stringValue="诊断完成：\(results.filter{($0["click_delta"] as? Int)==1}.count) 种投递触发按钮。"
            } catch {status.stringValue="诊断中止：\((error as? NodeError)?.rawValue ?? "fixture_unavailable")"}
            let data=try? JSONSerialization.data(withJSONObject:["post_event_access":CGPreflightPostEventAccess(),"stage":stage,"results":results])
            try? data?.write(to:resultURL,options:.atomic)
            busy=false;start.isEnabled=true
        }
    }
}
@main struct MacMouseDeliveryProbe {
    static func main() {
        let app=NSApplication.shared;app.setActivationPolicy(.regular)
        MainActor.assumeIsolated {let probe=MouseProbe();app.delegate=probe;withExtendedLifetime(probe){app.run()}}
    }
}
