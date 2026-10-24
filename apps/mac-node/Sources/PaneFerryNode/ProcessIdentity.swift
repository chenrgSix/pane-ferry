import Foundation
import Darwin

/// NSRunningApplication.launchDate is nil for real accessory helpers (including
/// Feishu's image viewer). Pin the kernel start time and executable instead.
struct ProcessIdentity: Hashable {
    let pid: pid_t, started: UInt64, executable: String
    static func read(_ pid: pid_t) -> (identity: ProcessIdentity, parent: pid_t)? {
        var info = proc_bsdinfo()
        guard proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, &info, Int32(MemoryLayout.size(ofValue: info))) == MemoryLayout.size(ofValue: info),
              info.pbi_uid == geteuid() else { return nil }
        var path = [CChar](repeating: 0, count: 4 * Int(MAXPATHLEN))
        guard proc_pidpath(pid, &path, UInt32(path.count)) > 0 else { return nil }
        return (ProcessIdentity(pid: pid, started: info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec,
            executable: URL(fileURLWithPath: String(cString: path)).resolvingSymlinksInPath().path), pid_t(info.pbi_ppid))
    }
    var current: Bool { Self.read(pid)?.identity == self }
    func descends(from root: ProcessIdentity) -> Bool {
        var current = self, seen = Set<pid_t>()
        for _ in 0..<32 {
            if current == root { return root.current }
            guard seen.insert(current.pid).inserted, let actual = Self.read(current.pid), actual.identity == current,
                  actual.parent > 1, let next = Self.read(actual.parent)?.identity, next.started <= current.started else { return false }
            current = next
        }
        return false
    }
    func belongs(to app: URL, root: ProcessIdentity) -> Bool {
        let directory = app.resolvingSymlinksInPath().standardized.path + "/"
        return (executable == root.executable || executable.hasPrefix(directory)) && descends(from: root)
    }
}
