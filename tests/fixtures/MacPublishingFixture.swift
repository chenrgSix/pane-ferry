import AppKit
final class FlippedView:NSView { override var isFlipped:Bool {true} }
final class Fixture:NSObject,NSApplicationDelegate,NSTextViewDelegate {
    var window:NSWindow!,text:NSTextView!,button:NSButton!,count=0
    var inputEvents=[[String:Any]]()
    let state:URL
    init(_ state:URL) {self.state=state}
    func applicationDidFinishLaunching(_ notification:Notification) {
        let menu=NSMenu(),application=NSMenuItem(),actions=NSMenu()
        actions.addItem(withTitle:"Quit RemoteApp Mac Test",action:#selector(NSApplication.terminate(_:)),keyEquivalent:"q")
        application.submenu=actions;menu.addItem(application);NSApplication.shared.mainMenu=menu
        let edit=NSMenuItem(title:"Edit",action:nil,keyEquivalent:""),editActions=NSMenu(title:"Edit")
        for (title,selector,key) in [("Cut",#selector(NSText.cut(_:)),"x"),("Copy",#selector(NSText.copy(_:)),"c"),("Paste",#selector(NSText.paste(_:)),"v"),("Select All",#selector(NSText.selectAll(_:)),"a")] {
            editActions.addItem(withTitle:title,action:selector,keyEquivalent:key)
        }
        edit.submenu=editActions;menu.addItem(edit)
        NSEvent.addLocalMonitorForEvents(matching:[.leftMouseDown,.leftMouseUp,.keyDown,.keyUp]) { [weak self] event in
            guard let self=self else {return event}
            self.inputEvents.append(["type":event.type.rawValue,"window":event.windowNumber,"x":event.locationInWindow.x,"y":event.locationInWindow.y,"key":event.keyCode,"flags":event.modifierFlags.rawValue,"active":NSApplication.shared.isActive])
            self.inputEvents=Array(self.inputEvents.suffix(16));self.write();return event
        }
        window=NSWindow(contentRect:NSRect(x:180,y:180,width:800,height:560),styleMask:[.titled,.closable,.resizable,.miniaturizable],backing:.buffered,defer:false)
        window.title="LAN RemoteApp Mac 发布测试";window.minSize=NSSize(width:400,height:300)
        let content=FlippedView(frame:NSRect(x:0,y:0,width:800,height:560));window.contentView=content
        button=NSButton(title:"点击计数 0",target:self,action:#selector(clicked));button.frame=NSRect(x:24,y:24,width:160,height:36);content.addSubview(button)
        let scroll=NSScrollView(frame:NSRect(x:24,y:90,width:740,height:430));scroll.autoresizingMask=[.width,.height]
        text=NSTextView(frame:scroll.bounds);text.isRichText=false;text.font=NSFont.systemFont(ofSize:20);text.delegate=self
        text.isVerticallyResizable=true;text.autoresizingMask=[.width];text.textContainer?.widthTracksTextView=true
        scroll.documentView=text;scroll.hasVerticalScroller=true;content.addSubview(scroll)
        window.makeKeyAndOrderFront(nil);NSApplication.shared.activate(ignoringOtherApps:true);write()
    }
    @objc func clicked() {count+=1;button.title="点击计数 \(count)";text.string="点击计数 \(count)";write()}
    func textDidChange(_ notification:Notification) {write()}
    func write() {
        let value:[String:Any]=["pid":ProcessInfo.processInfo.processIdentifier,"clicks":count,"text":text?.string ?? "","window_number":window.windowNumber,"input_events":inputEvents]
        if let data=try? JSONSerialization.data(withJSONObject:value) {try? data.write(to:state,options:.atomic)}
    }
}
@main struct MacPublishingFixture {
    static func main() {
        guard CommandLine.arguments.count==2 else {exit(2)}
        let app=NSApplication.shared;app.setActivationPolicy(.regular)
        let fixture=Fixture(URL(fileURLWithPath:CommandLine.arguments[1]));app.delegate=fixture
        withExtendedLifetime(fixture){app.run()}
    }
}
