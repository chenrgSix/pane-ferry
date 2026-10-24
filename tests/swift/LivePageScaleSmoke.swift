// Opt-in: an explicitly selected, prepared ChatGPT session; changes only its page lease.
import AppKit
@main struct LivePageScaleSmoke {
 static func main() throws {
  _ = NSApplication.shared
  NSApp.setActivationPolicy(.regular);NSApp.finishLaunching()
  let options = try Options.parse(Array(CommandLine.arguments.dropFirst()))
  let board = NSPasteboard.withUniqueName(); defer {board.releaseGlobally()}
  let app = AppDelegate(options:options,clipboard:board)
  app.terminateHost = {}
  defer {app.applicationWillTerminate(Notification(name:NSApplication.willTerminateNotification));for h in app.windows.values {h.closeLocally()}}
  var stage=0
  func until(_ condition:()->Bool) {
   stage += 1; let deadline=Date().addingTimeInterval(25)
   while !condition(),Date()<deadline {RunLoop.current.run(until:Date().addingTimeInterval(0.01));app.pump();while let event=NSApp.nextEvent(matching:.any,until:.distantPast,inMode:.default,dequeue:true) {NSApp.sendEvent(event)}}
   precondition(condition(),"live page-scale stage \(stage) timed out")
  }
  func settle() {let end=Date().addingTimeInterval(1.5);while Date()<end {RunLoop.current.run(until:Date().addingTimeInterval(0.01));app.pump();while let event=NSApp.nextEvent(matching:.any,until:.distantPast,inMode:.default,dequeue:true) {NSApp.sendEvent(event)}}}
  app.applicationDidFinishLaunching(Notification(name:NSApplication.didFinishLaunchingNotification))
  until {app.ready && app.windows.values.contains {$0.surface.image != nil}}
  let host=app.windows.values.first {$0.surface.image != nil}!
  NSApp.activate(ignoringOtherApps:true);host.window.makeKeyAndOrderFront(nil);settle()
  until {NSApp.keyWindow === host.window}
  precondition(host.pageScaleSupported && host.pageScale==1)
  let width=host.surface.remoteWidth,height=host.surface.remoteHeight,frame=host.window.frame
  let menu=NSMenuItem(title:"scale",action:#selector(AppDelegate.togglePageScale),keyEquivalent:"")
  precondition(app.validateMenuItem(menu) && menu.state == .off)
  app.togglePageScale()
  until {host.pageScale==2 && host.rendering.scale==2 && host.surface.image?.width==width*2 && host.surface.image?.height==height*2}
  settle();precondition(host.window.frame==frame && app.validateMenuItem(menu) && menu.state == .on)
  app.togglePageScale()
  until {host.pageScale==1 && host.rendering.scale==1 && host.surface.image?.width==width && host.surface.image?.height==height}
  app.togglePageScale();until {host.pageScale==2 && host.surface.image?.width==width*2}
  app.transport.disconnect();until {app.recovering}
  until {app.connected && !app.recovering && host.pageScale==1 && host.surface.image?.width==width && host.surface.image?.height==height}
  precondition(app.windows[host.id] === host && host.window.frame==frame)
  print("PASS: actual AppKit menu, H.264 \(width)x\(height) -> \(width*2)x\(height*2), same NSWindow/frame, explicit disable and automatic reconnect restore")
 }
}
