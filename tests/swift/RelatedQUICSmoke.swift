import AppKit

@main struct RelatedQUICSmoke {
    static func main() throws {
        _ = NSApplication.shared
        var options=try Options.parse(Array(CommandLine.arguments.dropFirst()))
        let board=NSPasteboard.withUniqueName();defer { board.releaseGlobally() }
        let app=AppDelegate(options:options,clipboard:board)
        defer {
            app.applicationWillTerminate(Notification(name:NSApplication.willTerminateNotification))
            for host in app.windows.values { host.closeLocally() }
        }
        func until(_ condition:()->Bool) {
            let end=Date().addingTimeInterval(20)
            while !condition(),Date()<end { RunLoop.current.run(until:Date().addingTimeInterval(0.01));app.pump() }
            precondition(condition(),"related QUIC live check timed out")
        }
        app.applicationDidFinishLaunching(Notification(name:NSApplication.didFinishLaunchingNotification))
        until { app.ready && app.windows.count==2 && app.windows.values.allSatisfy{$0.surface.image != nil} }
        let initial=Set(app.windows.keys)
        let root=app.windows.values.first{app.parents[$0.id]==nil}!
        let sid=app.session!
        func send(_ id:UInt32,_ type:String,_ fields:[String:Any]=[:]) {
            var value=fields;value["type"]=type;value["window_id"]=id;app.send(value)
        }
        func key(_ id:UInt32,_ symbol:UInt32,_ down:Bool) { send(id,"input.key",["keysym":symbol,"pressed":down]) }
        func copy(_ id:UInt32,_ text:String) {
            key(id,0xffe3,true);key(id,97,true);key(id,97,false);key(id,0xffe3,false)
            send(id,"clipboard.copy",["request_id":UUID().uuidString])
            until { !app.clipboardBusy && board.string(forType:.string)==text }
        }
        key(root.id,0xffbf,true);key(root.id,0xffbf,false)
        until { app.windows.count==3 && app.windows.values.allSatisfy{$0.surface.image != nil} }
        let child=app.windows.values.first{!initial.contains($0.id)}!
        precondition(child.relatedWindow && app.parents[child.id]==root.id && child.window.parent===root.window)
        precondition(child.window.subtitle.contains("H.264"))
        send(child.id,"input.text.commit",["text":"跨进程预览 QUIC🙂"]);copy(child.id,"跨进程预览 QUIC🙂")
        for _ in 0..<2 {
            let previous=child.surface.image
            app.transport.disconnect();until { app.recovering }
            until { app.connected && !app.recovering && child.surface.image != nil && child.surface.image !== previous }
            precondition(app.session==sid && app.windows[root.id]===root && app.windows[child.id]===child)
        }
        child.window.performClose(nil)
        until { app.windows[child.id]==nil }
        precondition(app.ready && app.windows[root.id]===root)
        send(root.id,"input.text.commit",["text":"主窗口仍正常🙂"]);copy(root.id,"主窗口仍正常🙂")
        print("PASS: real Ubuntu child process -> QUIC -> AppKit H264; automatic related window, scoped Unicode/copy, two reconnects retaining native windows, child close preserves parent")
    }
}
