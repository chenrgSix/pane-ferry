import AppKit

@main struct ApplicationGroupSmoke {
    static func until(_ condition: () -> Bool) {
        let deadline = Date().addingTimeInterval(12)
        while !condition(), Date() < deadline {
            RunLoop.main.run(until: Date().addingTimeInterval(0.01))
            while let event = NSApp.nextEvent(matching: .any, until: .distantPast, inMode: .default, dequeue: true) { NSApp.sendEvent(event) }
        }
        precondition(condition(), "session group timed out")
    }
    static func main() throws {
        let app = NSApplication.shared
        let root = URL(fileURLWithPath: CommandLine.arguments[1])
        let groupID = CommandLine.arguments[2]
        let options = try Options.parse(["--native-peer", "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
            "--native-state", root.path, "--native-helper", root.appendingPathComponent("bridge").path,
            "--session", "one", "--fps", "30", "--application-group", groupID, "--source-target", "window-one"])
        let group = try ApplicationGroup(options: options, icon: nil)
        var exits = 0
        group.terminateApplication = { exits += 1 }
        var boards: [NSPasteboard] = []
        group.makeSession = { options, icon in
            let board = NSPasteboard.withUniqueName(); boards.append(board)
            return AppDelegate(options: options, clipboard: board, applicationIcon: icon)
        }
        app.delegate = group
        group.applicationDidFinishLaunching(Notification(name: NSApplication.didFinishLaunchingNotification))
        app.finishLaunching()
        precondition(group.attach("two", target: "window-two", fps: 60, pictureMode: .source, background: true))
        precondition(group.attach("three", target: "window-three", fps: 30, background: true))
        until { group.sessions.count == 3 && group.sessions.values.allSatisfy { $0.windows[42]?.surface.displayFrame != nil && $0.ready } }
        let one = group.sessions["one"]!, two = group.sessions["two"]!, three = group.sessions["three"]!
        let firstWindow = one.windows[42]!, secondWindow = two.windows[42]!, thirdWindow = three.windows[42]!
        precondition(!firstWindow.preserveSourceSize && secondWindow.preserveSourceSize && !thirdWindow.preserveSourceSize, "each attachment snapshots its own display preference")
        precondition(firstWindow !== secondWindow && secondWindow !== thirdWindow)
        precondition(group.attach("two", target: "window-two", fps: 30, background: true))
        precondition(group.sessions.count == 3 && group.sessions["two"] === two && two.requestedVideoFPS == 60,
                     "manual/automatic reattach must not duplicate or reconfigure a live session")
        precondition((try? String(contentsOf: root.appendingPathComponent("connections-two"), encoding: .utf8).trimmingCharacters(in: .whitespacesAndNewlines)) == "1")
        print("PASS: three independent RWP sessions, identical window IDs, one AppKit owner; reattach is idempotent")

        for (id, controller) in [("one", one), ("two", two), ("three", three)] {
            let host = controller.windows[42]!
            host.window.makeKeyAndOrderFront(nil); app.activate(ignoringOtherApps: true)
            until { NSApp.keyWindow === host.window && NSApp.mainMenu === controller.applicationMenu }
            host.surface.insertText("text-\(id)", replacementRange: NSRange(location: NSNotFound, length: 0))
            let rate = controller.rateChoices.first { $0.tag == (id == "two" ? 30 : 60) }!
            NSApp.sendAction(rate.action!, to: rate.target, from: rate)
            precondition(controller.requestedVideoFPS == rate.tag)
        }
        precondition(one.requestedVideoFPS == 60 && two.requestedVideoFPS == 30 && three.requestedVideoFPS == 60)
        print("PASS: focused-window menu and keyboard routing remain scoped to the corresponding session")
        for controller in [one, two, three] {
            controller.send(["type": "clipboard.copy", "window_id": UInt32(42), "request_id": "same-request"])
        }
        until { [("one", one), ("two", two), ("three", three)].allSatisfy { id, controller in
            controller.controllerClipboard.string(forType: .string) == "clipboard-\(id)"
        } }
        print("PASS: identical clipboard request/window IDs on parallel sessions retain independent results")

        try Data().write(to: root.appendingPathComponent("cut-two"))
        until { (try? String(contentsOf: root.appendingPathComponent("connections-two"), encoding: .utf8).trimmingCharacters(in: .whitespacesAndNewlines)) == "2" && two.ready }
        precondition(two.windows[42] === secondWindow && one.windows[42] === firstWindow && three.windows[42] === thirdWindow)
        precondition((try? String(contentsOf: root.appendingPathComponent("connections-one"), encoding: .utf8).trimmingCharacters(in: .whitespacesAndNewlines)) == "1")
        print("PASS: one transport reconnects in place while the other sessions keep their own windows/connections")

        let captureError = Incoming(object: ["type": "error", "code": "capture_failed"], image: nil)
        func errorBurst(_ session: String, _ controller: AppDelegate, number: Int, code: String = "capture_failed") throws {
            try Data(code.utf8).write(to: root.appendingPathComponent("errors-\(session)"))
            until { controller.windows[42]?.window.title.hasSuffix("error-burst-\(number)") == true }
        }
        try errorBurst("one", one, number: 1)
        let firstNotice = one.notice.current!.window
        try errorBurst("one", one, number: 2)
        precondition(NSApp.modalWindow == nil && firstNotice.isVisible && one.notice.current?.window === firstNotice,
                     "repeated remote errors must return immediately without stacking modal dialogs")
        precondition(two.notice.current == nil && three.notice.current == nil)
        one.notice.current!.buttons[0].performClick(nil)
        try errorBurst("one", one, number: 3)
        precondition(one.notice.current == nil && group.sessions["one"] === one && firstWindow.window.isVisible,
                     "acknowledging a repeated error must not reopen it or close a live source window")
        try errorBurst("one", one, number: 4, code: "different_failure")
        try errorBurst("two", two, number: 1)
        let secondNotice = two.notice.current!.window
        precondition(one.notice.current?.window !== secondNotice && secondNotice.isVisible)
        print("PASS: repeated errors are coalesced and acknowledged once, with independent modeless notices per session")

        firstWindow.window.performClose(nil)
        until { group.sessions["one"] == nil }
        precondition(exits == 0 && group.sessions.count == 2 && secondWindow.window.isVisible && thirdWindow.window.isVisible &&
                     one.notice.current == nil && two.notice.current?.window === secondNotice && secondNotice.isVisible,
                     "closing a window with an error must dismiss only that session's notice")
        one.receive(captureError)
        precondition(one.notice.current == nil, "late errors must not reopen a closed session")
        secondWindow.surface.insertText("after-close", replacementRange: NSRange(location: NSNotFound, length: 0))
        try Data().write(to: root.appendingPathComponent("end-two"))
        until { group.sessions["two"] == nil }
        precondition(exits == 0 && group.sessions["three"] === three && two.notice.current == nil && !secondNotice.isVisible)
        print("PASS: closing one native window and exiting a source session leave the remaining session operational")
        group.detach("three")
        until { group.sessions.isEmpty && exits == 1 }
        group.applicationWillTerminate(Notification(name: NSApplication.willTerminateNotification))
        for board in boards { board.releaseGlobally() }
        print("PASS: only the last session closes the application owner")
    }
}
