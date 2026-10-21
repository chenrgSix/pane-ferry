import AppKit
final class FlippedView:NSView { override var isFlipped:Bool {true} }
final class Fixture:NSObject,NSApplicationDelegate,NSTextViewDelegate {
    var window:NSWindow!,text:NSTextView!,button:NSButton!,count=0
    let state:URL
    init(_ state:URL) {self.state=state}
    func applicationDidFinishLaunching(_ notification:Notification) {
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
    @objc func clicked() {count+=1;button.title="点击计数 \(count)";write()}
    func textDidChange(_ notification:Notification) {write()}
    func write() {
        let value:[String:Any]=["pid":ProcessInfo.processInfo.processIdentifier,"clicks":count,"text":text?.string ?? "","window_number":window.windowNumber]
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
