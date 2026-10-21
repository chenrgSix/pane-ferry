import Foundation
import Security
import Darwin

enum SSHPassword {
    static let service = "io.lanremoteapp.ssh-password"
    static func account(host: String, user: String, port: UInt16) -> String { "\(user)@\(host):\(port)" }
    static func query(_ account: String, keychain: SecKeychain? = nil) -> [String: Any] {
        var query: [String: Any] = [kSecClass as String: kSecClassGenericPassword,
                                   kSecAttrService as String: service, kSecAttrAccount as String: account]
        if let keychain = keychain { query[kSecMatchSearchList as String] = [keychain] }
        return query
    }
    static func contains(_ account: String, keychain: SecKeychain? = nil) -> Bool {
        var value = query(account, keychain: keychain)
        value[kSecReturnAttributes as String] = true
        value[kSecUseAuthenticationUI as String] = kSecUseAuthenticationUIFail
        var attributes: CFTypeRef?
        return SecItemCopyMatching(value as CFDictionary, &attributes) == errSecSuccess
    }
    static func read(_ account: String, keychain: SecKeychain? = nil) -> Data? {
        var value = query(account, keychain: keychain); value[kSecReturnData as String] = true
        var result: CFTypeRef?
        guard SecItemCopyMatching(value as CFDictionary, &result) == errSecSuccess else { return nil }
        return result as? Data
    }
    static func save(_ password: Data, account: String, keychain: SecKeychain? = nil) -> Bool {
        guard !password.isEmpty, password.count <= 4096, !password.contains(0), !password.contains(10), !password.contains(13) else { return false }
        let value = query(account, keychain: keychain)
        let status = SecItemUpdate(value as CFDictionary, [kSecValueData as String: password] as CFDictionary)
        if status == errSecSuccess { return true }
        guard status == errSecItemNotFound else { return false }
        var added = value; added[kSecValueData as String] = password
        if let keychain = keychain { added.removeValue(forKey:kSecMatchSearchList as String); added[kSecUseKeychain as String] = keychain }
        return SecItemAdd(added as CFDictionary, nil) == errSecSuccess
    }
    /// SSH starts this executable directly as its askpass helper. Only a
    /// password prompt for the configured endpoint may receive the password.
    static func askpass(prompt: String, account: String, keychain: SecKeychain? = nil) -> Data? {
        let lower = prompt.lowercased()
        guard lower.contains("password"), !lower.contains("passphrase"), !lower.contains("yes/no"),
              let password = read(account, keychain: keychain), !password.isEmpty else { return nil }
        return password
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
    guard let command = arguments.first, ["--credential-store", "--credential-check"].contains(command) else { return nil }
    guard arguments.count == 4, let port = UInt16(arguments[3]), port > 0,
          arguments[1].count <= 253, arguments[1].range(of:"^[A-Za-z0-9][A-Za-z0-9.-]*$", options:.regularExpression) != nil,
          arguments[1].split(separator:".", omittingEmptySubsequences:false).allSatisfy({ !$0.isEmpty && $0.count <= 63 && !$0.hasPrefix("-") && !$0.hasSuffix("-") }),
          arguments[2].range(of:"^[A-Za-z0-9_][A-Za-z0-9_-]{0,63}$", options:.regularExpression) != nil else { return 2 }
    let account = SSHPassword.account(host: arguments[1], user: arguments[2], port: port)
    if command == "--credential-check" { return SSHPassword.contains(account) ? 0 : 1 }
    guard let data = try? FileHandle.standardInput.read(upToCount: 4097), data.count <= 4096 else { return 2 }
    return SSHPassword.save(data, account: account) ? 0 : 1
}
