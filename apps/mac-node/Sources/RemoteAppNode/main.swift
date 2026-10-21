import AppKit
import ScreenCaptureKit
import Darwin
// Readiness probes intentionally connect and close without a hello. Closed
// peers must only end their connection, never terminate the graphical service.
signal(SIGPIPE,SIG_IGN)

final class Client {
    let id=UUID(),wire:Wire,local:Bool
    var session:PublishedSession?,capture:WindowCapture?,video=false
    init(_ wire:Wire,local:Bool) { self.wire=wire;self.local=local }
    func detach() { capture?.stop();capture=nil;if let session=session,session.controller==id { session.release();session.controller=nil };session=nil }
}
@MainActor final class Node: NSObject, NSApplicationDelegate {
    let publisher:Publisher
    let instanceLock:Int32
    var clients=[UUID:Client](),timer:Timer?
    init(_ directory:URL) throws {
        try privateDirectory(directory)
        instanceLock=open(directory.appendingPathComponent(".service.lock").path,O_RDWR|O_CREAT|O_NOFOLLOW,0o600)
        guard instanceLock>=0,flock(instanceLock,LOCK_EX|LOCK_NB)==0 else { throw NodeError.invalid_request }
        publisher=try Publisher(directory:directory);super.init()
    }
    func applicationDidFinishLaunching(_ notification:Notification) {
        do { try listen("node.sock",local:false);try listen("management.sock",local:true) }
        catch { fputs("LAN RemoteApp: local service already running or unavailable\n",stderr);NSApplication.shared.terminate(nil);return }
        timer=Timer.scheduledTimer(withTimeInterval:0.5,repeats:true){ [weak self] _ in
            MainActor.assumeIsolated { self?.checkSessions() }
        }
    }
    func listen(_ name:String,local:Bool) throws {
        let path=publisher.directory.appendingPathComponent(name).path
        if let fd=try? unixSocket(path,listening:false) { close(fd);throw NodeError.invalid_request }
        unlink(path)
        let listener=try unixSocket(path,listening:true)
        let node=self
        DispatchQueue.global().async {
            while true {
                let fd=accept(listener,nil,nil);if fd<0 { continue }
                var uid:uid_t=0,gid:gid_t=0
                guard getpeereid(fd,&uid,&gid)==0,uid==geteuid() else { close(fd);continue }
                let wire=Wire(fd),client=Client(wire,local:local)
                DispatchQueue.global().async {
                    do {
                        guard let hello=try wire.read(),hello["type"] as? String == "hello",hello.count==2 else { throw NodeError.invalid_request }
                        wire.send(["type":"hello","capabilities":["native.h264","node.info","platform.macos","input.key.logical","input.text.commit","clipboard.inline","session.resume","session.list","app.discovery","window.list"]])
                        while let request=try wire.read() {
                            let finished=DispatchSemaphore(value:0)
                            Task { @MainActor in
                                defer { finished.signal() }
                                if node.clients[client.id]==nil {
                                    guard node.clients.count<16 else { wire.finish();return }
                                    node.clients[client.id]=client
                                }
                                do { try await node.handle(request,client) }
                                catch { wire.send(["type":"error","code":(error as? NodeError)?.rawValue ?? "capture_failed"],session:client.session?.id ?? "") }
                            }
                            finished.wait()
                        }
                    } catch { wire.send(["type":"error","code":"invalid_request"]) }
                    wire.finish()
                    Task { @MainActor in client.detach();node.clients.removeValue(forKey:client.id) }
                }
            }
        }
    }
    func checkSessions() {
        for client in clients.values {
            guard let session=client.session else { continue }
            if !publisher.sharing || !session.valid() || desktopLocked() || !CGPreflightScreenCaptureAccess() {
                client.wire.send(["type":"error","code":!publisher.sharing ? "sharing_disabled" : desktopLocked() ? "desktop_locked" : "session_exited"],session:session.id)
                client.detach();client.wire.finish()
            }
        }
    }
    func attach(_ session:PublishedSession,_ client:Client,snapshot:Bool) async throws {
        guard publisher.sharing else { throw NodeError.sharing_disabled }
        guard session.valid() else { throw NodeError.session_exited }
        guard session.controller==nil else { throw NodeError.session_controlled }
        guard client.session==nil,client.video else { throw NodeError.invalid_request }
        guard CGPreflightScreenCaptureAccess() else { throw NodeError.screen_recording_required }
        session.controller=client.id;client.session=session
        let list=try await publisher.scan()
        guard publisher.sharing,session.controller==client.id else { client.detach();throw NodeError.sharing_disabled }
        guard let window=list.first(where:{$0.id==session.target.id})?.window else { client.detach();throw NodeError.session_exited }
        let capture=WindowCapture(wire:client.wire,session:session.id,windowID:window.windowID,width:session.width,height:session.height,revision:session.revision)
        client.capture=capture
        client.wire.send(snapshot ? ["type":"session.snapshot","windows":[session.metadata]] : session.metadata,session:session.id)
        do { try await capture.start(window) } catch { client.detach();throw NodeError.capture_failed }
    }
    func handle(_ request:[String:Any],_ client:Client) async throws {
        guard let type=request["type"] as? String else { throw NodeError.invalid_request }
        // Strict request schema. Protocol extensions are negotiated, not silently
        // treated as input or executable/path arguments.
        let schemas:[String:Set<String>] = [
            "node.info":[],"permissions.request":[],"sharing.set":["enabled"],"app.list":[],"window.list":[],"session.list":[],
            "video.enable":["codec"],"app.launch":["app_id","request_id","backend","attach","scale_factor"],"session.resume":["session_id"],"session.detach":[],
            "window.focus":["window_id"],"window.close.request":["window_id"],"window.configure":["window_id","width","height"],
            "input.pointer":["window_id","x","y","button","pressed"],"input.key.logical":["window_id","key","pressed"],
            "input.text.commit":["window_id","text"],"clipboard.paste":["window_id","text"],"clipboard.copy":["window_id","request_id"],
            "clipboard.cut":["window_id","request_id"],"clipboard.inline.paste":["window_id","bundle"],"clipboard.inline.copy":["window_id","request_id","cut"]]
        guard let schema=schemas[type],Set(request.keys).subtracting(["type","version"]).isSubset(of:schema) else { throw NodeError.invalid_request }
        switch type {
        case "node.info":client.wire.send(publisher.info())
        case "permissions.request":
            guard client.local else { throw NodeError.invalid_request }
            _ = CGRequestScreenCaptureAccess()
            _ = AXIsProcessTrustedWithOptions([kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String:true] as CFDictionary)
            client.wire.send(publisher.info())
        case "sharing.set":
            guard client.local,let enabled=request["enabled"] as? Bool else { throw NodeError.invalid_request }
            try savePrivate(["enabled":enabled],to:publisher.directory.appendingPathComponent("sharing.json"));publisher.sharing=enabled
            checkSessions();client.wire.send(publisher.info())
        case "app.list":
            guard publisher.sharing else { throw NodeError.sharing_disabled }
            publisher.apps=installedApps()
            client.wire.send(["type":"app.list","apps":publisher.apps.values.sorted{$0.name<$1.name}.map{$0.publicValue},"discovery_supported":true,"discovery_error":NSNull()])
        case "window.list":
            let targets=try await publisher.scan()
            let windows=targets.map{target -> [String:Any] in
                let session=publisher.sessions.values.first{$0.target.id==target.id && $0.valid()}
                return ["id":target.id,"window_id":target.window.windowID,"name":target.app.name,"title":String((target.window.title ?? "").prefix(512)),
                    "icon_png":icon(target.app.url) ?? NSNull(),"minimized":!target.window.isOnScreen,"controlled":session?.controller != nil,"session_id":session?.id ?? NSNull()]
            }
            client.wire.send(["type":"window.list","windows":windows,"unavailable_reason":NSNull()])
        case "session.list":
            let sessions=publisher.sessions.values.filter{$0.valid()}.map{["session_id":$0.id,"app_id":$0.appID,"backend":"native","display":0,"controlled":$0.controller != nil] as [String:Any]}
            client.wire.send(["type":"session.list","sessions":sessions])
        case "video.enable":
            guard client.session==nil,request["codec"] as? String == "h264" else { throw NodeError.invalid_request }
            client.video=true;client.wire.send(["type":"video.ready","codec":"h264"])
        case "app.launch":
            guard let app=request["app_id"] as? String,let id=request["request_id"] as? String,request["backend"] as? String == "native" else { throw NodeError.invalid_request }
            let session=try await publisher.launch(app,id)
            client.wire.send(["type":"session.started","backend":"native","display":0,"scale_factor":2],session:session.id)
            if request["attach"] as? Bool != false { try await attach(session,client,snapshot:false) }
        case "session.resume":
            guard let id=request["session_id"] as? String,identifier(id),let session=publisher.sessions[id] else { throw NodeError.session_expired }
            try await attach(session,client,snapshot:true)
        case "session.detach":client.detach();client.wire.finish()
        default:
            guard publisher.sharing else { throw NodeError.sharing_disabled }
            guard let session=client.session,session.controller==client.id,let window=request["window_id"] as? UInt32,window==session.target.window.windowID,session.valid() else { throw NodeError.window_unavailable }
            try session.ensureInput()
            switch type {
            case "window.focus":try session.focus()
            case "window.close.request":
                let window=try session.axWindow();var button:CFTypeRef?
                guard AXUIElementCopyAttributeValue(window,kAXCloseButtonAttribute as CFString,&button) == .success,let button=button,CFGetTypeID(button)==AXUIElementGetTypeID() else { throw NodeError.window_unavailable }
                AXUIElementPerformAction(button as! AXUIElement,kAXPressAction as CFString)
            case "window.configure":
                guard let width=request["width"] as? Int,let height=request["height"] as? Int,dimensions(width,height) else { throw NodeError.invalid_request }
                let window=try session.axWindow();var size=CGSize(width:max(160,width/2),height:max(100,height/2))
                guard let value=AXValueCreate(.cgSize,&size),AXUIElementSetAttributeValue(window,kAXSizeAttribute as CFString,value) == .success else { throw NodeError.window_unavailable }
                _ = session.valid();session.width=width;session.height=height;session.revision+=1
                var meta=session.metadata;meta["type"]="window.metadata";client.wire.send(meta,session:session.id)
                try await client.capture?.update(width:width,height:height,revision:session.revision)
            case "input.pointer":
                guard let x=request["x"] as? Int,let y=request["y"] as? Int,let button=request["button"] as? Int,let pressed=request["pressed"] as? Bool else { throw NodeError.invalid_request }
                try session.pointer(x,y,button,pressed)
            case "input.key.logical":
                guard let key=request["key"] as? String,key.count<=32,let pressed=request["pressed"] as? Bool else { throw NodeError.invalid_request }
                try session.keyEvent(key,pressed)
            case "input.text.commit":guard let text=request["text"] as? String else { throw NodeError.invalid_request };try session.text(text)
            case "clipboard.paste":
                guard let text=request["text"] as? String else { throw NodeError.clipboard_invalid }
                try writeClipboard(["kind":"text","text":text],directory:publisher.directory);try session.shortcut(9)
                try await Task.sleep(nanoseconds:80_000_000)
            case "clipboard.inline.paste":
                guard let body=request["bundle"] as? [String:Any] else { throw NodeError.clipboard_invalid }
                try writeClipboard(body,directory:publisher.directory);try session.shortcut(9)
                try await Task.sleep(nanoseconds:80_000_000)
            case "clipboard.copy","clipboard.cut","clipboard.inline.copy":
                guard let id=request["request_id"] as? String,identifier(id) else { throw NodeError.invalid_request }
                let generation=NSPasteboard.general.changeCount
                try session.shortcut(type=="clipboard.cut" || request["cut"] as? Bool == true ? 7:8)
                for _ in 0..<40 {
                    try await Task.sleep(nanoseconds:10_000_000)
                    if NSPasteboard.general.changeCount != generation { break }
                }
                guard NSPasteboard.general.changeCount != generation else { throw NodeError.clipboard_copy_failed }
                guard publisher.sharing,session.controller==client.id,session.valid(),
                      NSWorkspace.shared.frontmostApplication?.processIdentifier==session.target.application.processIdentifier else { throw NodeError.clipboard_copy_failed }
                let bundle=try readClipboard()
                if type=="clipboard.inline.copy" { client.wire.send(["type":"clipboard.inline.data","window_id":window,"request_id":id,"bundle":bundle],session:session.id) }
                else { guard let text=bundle["text"] as? String else { throw NodeError.clipboard_invalid };client.wire.send(["type":"clipboard.data","window_id":window,"request_id":id,"text":text],session:session.id) }
            default:throw NodeError.invalid_request
            }
        }
    }
}

let directory=FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/io.lanremoteapp.launcher/Node")
let app=NSApplication.shared
app.setActivationPolicy(.accessory)
MainActor.assumeIsolated { do {
    let node=try Node(directory)
    app.delegate=node
    withExtendedLifetime(node) { app.run() }
} catch { fputs("LAN RemoteApp: service configuration unavailable\n",stderr);exit(1) } }
