import AppKit

@main struct NetworkLatencySmoke {
    static func until(_ condition: () -> Bool, timeout: TimeInterval = 3) {
        let deadline = Date().addingTimeInterval(timeout)
        while !condition(), Date() < deadline { RunLoop.main.run(until: Date().addingTimeInterval(0.01)) }
        precondition(condition(), "latency monitor timed out")
    }
    static func main() throws {
        _ = NSApplication.shared
        let route = NativeRoute(peer: "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa", state: CommandLine.arguments[1], helper: "/usr/bin/false")
        if CommandLine.arguments.last == "slow" {
            let monitor = NetworkLatency(route: route)
            var ticks = 0
            let timer = Timer(timeInterval: 0.01, repeats: true) { _ in ticks += 1 }
            RunLoop.main.add(timer, forMode: .common)
            let start = ProcessInfo.processInfo.systemUptime
            monitor.refresh()
            until { monitor.state == .unavailable }
            timer.invalidate()
            let elapsed = ProcessInfo.processInfo.systemUptime - start
            precondition(elapsed < 1.6 && ticks >= 30, "stalled status must time out off the UI thread: \(elapsed), \(ticks)")
            print("PASS: unavailable status is bounded to one second; AppKit run loop keeps advancing")
            return
        }
        let sample = try NetworkLatency.readStatus(route)
        precondition(sample == 24.75, "read the selected peer, never the first unrelated device")
        let missing = try NetworkLatency.parse(Data("{\"peers\":[{\"device_id\":\"\(route.peer)\",\"connected\":true}]}".utf8), peer: route.peer)
        precondition(missing == nil, "older network components must not invent zero latency")
        for value in ["null", "true", "-1", "\"42\""] {
            let data = Data("{\"peers\":[{\"device_id\":\"\(route.peer)\",\"connected\":true,\"rtt_ms\":\(value)}]}".utf8)
            precondition(try! NetworkLatency.parse(data, peer: route.peer) == nil)
        }
        let closed = Data("{\"peers\":[{\"device_id\":\"\(route.peer)\",\"connected\":false,\"rtt_ms\":99}]}".utf8)
        precondition(try! NetworkLatency.parse(closed, peer: route.peer) == nil)
        print("PASS: fragmented local status, device isolation, legacy/malformed/closed samples")

        let monitor = NetworkLatency.monitor(route)
        precondition(monitor === NetworkLatency.monitor(route), "sessions share one poller per device")
        let other = NativeRoute(peer: "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb", state: route.state, helper: route.helper)
        precondition(monitor !== NetworkLatency.monitor(other))
        var now: TimeInterval = 100
        var count = 0
        monitor.clock = { now }
        monitor.read = { _ in count += 1; return 42.25 }
        monitor.refresh(); monitor.refresh()
        until { monitor.state == .milliseconds(42.25) }
        precondition(count == 1)
        now += 1; monitor.refresh(); precondition(count == 1)
        now += 1; monitor.read = { _ in count += 1; return nil }; monitor.refresh()
        until { monitor.state == .unavailable }
        now += 2; monitor.read = { _ in count += 1; return 19 }; monitor.refresh()
        until { monitor.state == .milliseconds(19) }
        now += 7; precondition(monitor.state == .unavailable, "stale samples must expire")
        precondition(NetworkLatencyState.milliseconds(0.2).text == "网络往返 <1 ms")
        let host = HostedWindow(id: 1, width: 640, height: 480, revision: 1, scale: 1, popup: false, disconnect: {}) { _ in }
        host.networkLatency = .milliseconds(19.25); host.updateSubtitle()
        precondition(host.window.subtitle.contains("网络往返 19 ms"))
        host.networkLatency = .unavailable; host.updateSubtitle()
        precondition(host.window.subtitle.contains("网络往返 暂无") && !host.window.subtitle.contains("19 ms"))
        host.closeLocally()
        print("PASS: shared/rate-limited asynchronous polling, recovery, stale expiry and honest native title labels")
    }
}
