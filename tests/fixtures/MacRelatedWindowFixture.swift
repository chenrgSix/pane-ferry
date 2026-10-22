import AppKit

/// A main process with two unrelated windows and an accessory child process
/// with an unparented preview, matching the observed Feishu ownership pattern.
final class RelatedFixture: NSObject, NSApplicationDelegate, NSTextViewDelegate {
    let state: URL, helper: Bool
    var windows=[NSWindow](), fields=[NSTextView](), children=[Process](), timer: Timer?
    var events=[[String:Any]]()
    var termination: DispatchSourceSignal?
    var eventMonitor: Any?
    init(state: URL, helper: Bool) { self.state=state;self.helper=helper }
    func applicationDidFinishLaunching(_ notification: Notification) {
        signal(SIGTERM,SIG_IGN)
        termination=DispatchSource.makeSignalSource(signal:SIGTERM,queue:.main)
        termination?.setEventHandler { NSApplication.shared.terminate(nil) };termination?.resume()
        let menu=NSMenu(),appItem=NSMenuItem(),actions=NSMenu()
        actions.addItem(withTitle:"Quit",action:#selector(NSApplication.terminate(_:)),keyEquivalent:"q")
        appItem.submenu=actions;menu.addItem(appItem)
        let edit=NSMenuItem(title:"Edit",action:nil,keyEquivalent:""),editing=NSMenu()
        for (title,action,key) in [("Copy",#selector(NSText.copy(_:)),"c"),("Paste",#selector(NSText.paste(_:)),"v"),("Select All",#selector(NSText.selectAll(_:)),"a")] {
            editing.addItem(withTitle:title,action:action,keyEquivalent:key)
        }
        edit.submenu=editing;menu.addItem(edit);NSApplication.shared.mainMenu=menu
        for (index,title) in (helper ? ["窗渡 PaneFerry Related Preview"] : ["窗渡 PaneFerry Related Alpha","窗渡 PaneFerry Related Beta"]).enumerated() {
            let window=NSWindow(contentRect:NSRect(x:100+index*540,y:180,width:500,height:320),styleMask:[.titled,.closable,.resizable],backing:.buffered,defer:false)
            window.isReleasedWhenClosed=false;window.title=title
            let field=NSTextView(frame:NSRect(x:15,y:15,width:470,height:240));field.isRichText=false
            field.font=NSFont.systemFont(ofSize:22);field.delegate=self
            window.contentView?.addSubview(field)
            if !helper {
                let button=NSButton(title:"Open preview (F2)",target:self,action:#selector(openPreview))
                button.frame=NSRect(x:15,y:270,width:200,height:30);window.contentView?.addSubview(button)
            }
            windows.append(window);fields.append(field);window.makeKeyAndOrderFront(nil);window.makeFirstResponder(field)
        }
        eventMonitor=NSEvent.addLocalMonitorForEvents(matching:[.keyDown,.keyUp]) { [weak self] event in
            guard let self=self else { return event }
            self.events.append(["key":event.keyCode,"down":event.type == .keyDown,"flags":event.modifierFlags.rawValue,"active":NSApplication.shared.isActive])
            self.events=Array(self.events.suffix(20))
            if !self.helper && event.type == .keyDown && event.keyCode==120 { self.openPreview();return nil }
            return event
        }
        NSApplication.shared.activate(ignoringOtherApps:true)
        timer=Timer.scheduledTimer(withTimeInterval:0.05,repeats:true) { [weak self] _ in self?.write() };write()
    }
    @objc func openPreview() {
        guard !children.contains(where:{$0.isRunning}) else { return }
        let child=Process()
        child.executableURL=Bundle.main.bundleURL.appendingPathComponent("Contents/Helpers/Preview.app/Contents/MacOS/Preview")
        child.arguments=[state.appendingPathExtension("preview").path,"--helper"]
        try! child.run();children.append(child)
    }
    func textDidChange(_ notification:Notification) { write() }
    func write() {
        let value:[String:Any]=["pid":ProcessInfo.processInfo.processIdentifier,"children":children.filter{$0.isRunning}.map{$0.processIdentifier},"events":events,"windows":zip(windows,fields).filter{$0.0.isVisible}.map{["id":$0.0.windowNumber,"title":$0.0.title,"text":$0.1.string,"selection_length":$0.1.selectedRange().length] as [String:Any]}]
        if let data=try? JSONSerialization.data(withJSONObject:value) { try? data.write(to:state,options:.atomic) }
    }
    func applicationShouldTerminateAfterLastWindowClosed(_ sender:NSApplication) -> Bool { helper }
    func applicationWillTerminate(_ notification:Notification) {
        for child in children where child.isRunning { child.terminate() }
    }
}
@main struct MacRelatedWindowFixture {
    static func main() {
        guard CommandLine.arguments.count>=2 else { exit(2) }
        let helper=CommandLine.arguments.contains("--helper"),app=NSApplication.shared
        app.setActivationPolicy(helper ? .accessory:.regular)
        let fixture=RelatedFixture(state:URL(fileURLWithPath:CommandLine.arguments[1]),helper:helper);app.delegate=fixture
        withExtendedLifetime(fixture) { app.run() }
    }
}
