import Foundation
import Darwin

enum SSHPassword {
    struct Connection: Codable {
        let host: String, user: String, port: UInt16, password: String
    }
    struct Configuration: Codable {
        let version: Int
        let connections: [Connection]
    }
    static var configuration: URL {
        FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("io.lanremoteapp.launcher/ssh-connections.json")
    }
    static func account(host: String, user: String, port: UInt16) -> String { "\(user)@\(host):\(port)" }
    static func contains(_ account: String, configuration: URL = configuration) -> Bool {
        read(account, configuration: configuration) != nil
    }
    static func read(_ account: String, configuration: URL = configuration) -> Data? {
        guard let attributes = try? FileManager.default.attributesOfItem(atPath: configuration.path),
              attributes[.type] as? FileAttributeType == .typeRegular,
              let owner = attributes[.ownerAccountID] as? NSNumber, owner.uint32Value == geteuid(),
              let mode = attributes[.posixPermissions] as? NSNumber, mode.intValue & 0o077 == 0,
              let size = attributes[.size] as? NSNumber, size.intValue <= 1024 * 1024,
              let bytes = try? Data(contentsOf: configuration), bytes.count <= 1024 * 1024,
              let document = try? JSONDecoder().decode(Configuration.self, from: bytes),
              document.version == 1, document.connections.count <= 128 else { return nil }
        let matching = document.connections.filter { self.account(host:$0.host, user:$0.user, port:$0.port) == account }
        guard matching.count == 1, let record = matching.first else { return nil }
        let data = Data(record.password.utf8)
        guard !data.isEmpty, data.count <= 4096, !data.contains(0), !data.contains(10), !data.contains(13) else { return nil }
        return data
    }
    /// Only a password prompt for the configured endpoint receives its secret.
    static func askpass(prompt: String, account: String, configuration: URL = configuration) -> Data? {
        let lower = prompt.lowercased()
        guard lower.contains("password"), !lower.contains("passphrase"), !lower.contains("yes/no") else { return nil }
        return read(account, configuration: configuration)
    }
}

enum SSHAuthentication {
    static func executable(_ pid: pid_t) -> String? {
        var bytes = [CChar](repeating: 0, count: 4096)
        guard proc_pidpath(pid, &bytes, UInt32(bytes.count)) > 0 else { return nil }
        return String(cString: bytes)
    }
    static func trustedAskpassCaller() -> Bool {
        let parent = getppid()
        guard executable(parent) == "/usr/bin/ssh", let own = Bundle.main.executableURL else { return false }
        var info = proc_bsdinfo()
        guard proc_pidinfo(parent, PROC_PIDTBSDINFO, 0, &info, Int32(MemoryLayout<proc_bsdinfo>.size)) == MemoryLayout<proc_bsdinfo>.size else { return false }
        let launcher = Bundle.main.bundleURL.deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("Contents/MacOS/lan-remoteapp-launcher").path
        return [own.path, launcher].contains(executable(pid_t(info.pbi_ppid)) ?? "")
    }
    static func configure(_ process: Process, host: String, user: String, port: UInt16) -> [String] {
        let account = SSHPassword.account(host: host, user: user, port: port)
        var environment = ProcessInfo.processInfo.environment
        environment["LC_ALL"] = "C"
        if SSHPassword.contains(account), let helper = Bundle.main.executableURL {
            environment["SSH_ASKPASS"] = helper.path
            environment["SSH_ASKPASS_REQUIRE"] = "force"
            environment["REMOTEAPP_SSH_ASKPASS_ACCOUNT"] = account
            process.environment = environment
            return ["-S", "~/.ssh/lra-pw-%C", "-oBatchMode=no", "-oNumberOfPasswordPrompts=1", "-oControlMaster=auto", "-oControlPersist=10m"]
        }
        process.environment = environment
        return ["-S", "~/.ssh/lan-remoteapp-%C", "-oBatchMode=yes"]
    }
}

/// Non-UI modes run before AppKit starts; secrets only cross anonymous stdin/
/// stdout pipes between our helper and system SSH, never argv or configuration.
func runCredentialCommand(_ arguments: [String]) -> Int32? {
    if let account = ProcessInfo.processInfo.environment["REMOTEAPP_SSH_ASKPASS_ACCOUNT"] {
        guard SSHAuthentication.trustedAskpassCaller(), arguments.count == 1,
              let value = SSHPassword.askpass(prompt: arguments[0], account: account) else { return 1 }
        FileHandle.standardOutput.write(value); FileHandle.standardOutput.write(Data([10])); return 0
    }
    guard let command = arguments.first, command == "--credential-check" else { return nil }
    guard arguments.count == 4, let port = UInt16(arguments[3]), port > 0,
          arguments[1].count <= 253, arguments[1].range(of:"^[A-Za-z0-9][A-Za-z0-9.-]*$", options:.regularExpression) != nil,
          arguments[1].split(separator:".", omittingEmptySubsequences:false).allSatisfy({ !$0.isEmpty && $0.count <= 63 && !$0.hasPrefix("-") && !$0.hasSuffix("-") }),
          arguments[2].range(of:"^[A-Za-z0-9_][A-Za-z0-9_-]{0,63}$", options:.regularExpression) != nil else { return 2 }
    let account = SSHPassword.account(host: arguments[1], user: arguments[2], port: port)
    return SSHPassword.contains(account) ? 0 : 1
}
