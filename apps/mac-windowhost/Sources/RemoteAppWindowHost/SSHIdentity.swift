import Foundation

enum SSHIdentity {
    static func options(host: String, user: String, port: UInt16,
                        home: URL = FileManager.default.homeDirectoryForCurrentUser) -> [String] {
        let key = home.appendingPathComponent(".ssh/lan-remoteapp-keys/\(user)@\(host)-\(port)")
        let marker = key.appendingPathExtension("enabled")
        let regular = { (url: URL) -> Bool in
            guard let attributes = try? FileManager.default.attributesOfItem(atPath: url.path) else { return false }
            return attributes[.type] as? FileAttributeType == .typeRegular
        }
        guard regular(key), regular(marker) else { return ["-S", "~/.ssh/lan-remoteapp-%C"] }
        return ["-S", "~/.ssh/lan-remoteapp-auto-%C", "-i", key.path,
                "-oIdentitiesOnly=yes", "-oControlMaster=auto", "-oControlPersist=10m"]
    }
}
