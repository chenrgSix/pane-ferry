import Foundation

/// One rate controller per remote client. Acknowledgements refer to frames
/// painted by the receiver, including encoder, network and decoder delay.
final class VideoRateController {
    private let lock = NSLock()
    private var cap = 60
    private var target = 60
    private var feedback = false
    private var nextID: UInt64 = 1
    private var pending: [(id: UInt64, sent: TimeInterval)] = []
    private var delays: [TimeInterval] = []
    private var good = 0
    private var bad = 0
    private var lastChange = ProcessInfo.processInfo.systemUptime
    private var lastFeedback = ProcessInfo.processInfo.systemUptime

    func configure(cap: Int, feedback: Bool, now: TimeInterval = ProcessInfo.processInfo.systemUptime) {
        lock.withLock {
            self.cap = cap; target = cap; self.feedback = feedback
            pending.removeAll(); delays.removeAll(); good = 0; bad = 0
            lastChange = now; lastFeedback = now
        }
    }
    func configure(cap: Int, now: TimeInterval = ProcessInfo.processInfo.systemUptime) {
        lock.withLock {
            self.cap = cap; target = cap
            pending.removeAll(); delays.removeAll(); good = 0; bad = 0
            lastChange = now; lastFeedback = now
        }
    }
    func maximum() -> Int { lock.withLock { cap } }
    func desiredFPS(now: TimeInterval = ProcessInfo.processInfo.systemUptime) -> Int {
        lock.withLock { checkStall(now); return target }
    }
    func interval(now: TimeInterval = ProcessInfo.processInfo.systemUptime) -> TimeInterval {
        lock.withLock {
            checkStall(now)
            return 1.0 / Double(target)
        }
    }
    func sent(now: TimeInterval = ProcessInfo.processInfo.systemUptime) -> UInt64? {
        lock.withLock {
            guard feedback else { return nil }
            checkStall(now)
            let id = nextID
            nextID = nextID &+ 1
            if nextID == 0 { nextID = 1 }
            if pending.count == 128 { pending.removeFirst() }
            pending.append((id, now))
            return id
        }
    }
    @discardableResult func acknowledge(_ id: UInt64, now: TimeInterval = ProcessInfo.processInfo.systemUptime) -> Bool {
        lock.withLock {
            guard feedback, let index = pending.firstIndex(where: { $0.id == id }) else { return false }
            lastFeedback = now
            let delay = max(0, now - pending[index].sent)
            pending.removeFirst(index + 1)
            delays.append(delay)
            if delays.count > 24 { delays.removeFirst() }
            let excess = max(0, delay - (delays.min() ?? delay))
            if excess > 0.1 || pending.count > 4 {
                good = 0; bad += 1
                if bad >= 2 && now - lastChange >= 0.5 {
                    target = max(min(15, cap), target * 3 / 4)
                    lastChange = now; bad = 0
                    return true
                }
            } else if excess < 0.035 && pending.count <= 2 {
                bad = 0; good += 1
                if good >= 4 && now - lastChange >= 0.5 {
                    let previous = target
                    target = min(cap, target + 5)
                    lastChange = now; good = 0
                    return target != previous
                }
            } else { good = 0; bad = 0 }
            return false
        }
    }
    private func checkStall(_ now: TimeInterval) {
        guard feedback, !pending.isEmpty, now - lastFeedback >= 1, now - lastChange >= 1 else { return }
        target = max(min(15, cap), target / 2)
        lastChange = now; good = 0; bad = 0; pending.removeAll()
    }
}
