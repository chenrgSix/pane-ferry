import Foundation

@main
struct SSHPasswordSmoke {
    static func main() throws {
        let root = URL(fileURLWithPath: CommandLine.arguments[1])
        let file = root.appendingPathComponent("ssh-connections.json")
        precondition(!SSHAuthentication.trustedAskpassCaller(), "direct CLI cannot export saved credentials")
        let account = SSHPassword.account(host:"fixture.invalid", user:"fixture", port:22)
        let first = SSHPassword.Connection(host:"fixture.invalid", user:"fixture", port:22, password:"fixture-password-value")
        func write(_ entries: [SSHPassword.Connection], version: Int = 1) throws {
            let bytes = try JSONEncoder().encode(SSHPassword.Configuration(version:version, connections:entries))
            try bytes.write(to:file, options:.atomic)
            try FileManager.default.setAttributes([.posixPermissions:0o600], ofItemAtPath:file.path)
        }
        precondition(!SSHPassword.contains(account, configuration:file))
        try write([first])
        precondition(SSHPassword.contains(account, configuration:file))
        precondition(SSHPassword.read(account, configuration:file) == Data(first.password.utf8))
        precondition(SSHPassword.askpass(prompt:"fixture@fixture.invalid's password: ", account:account, configuration:file) == Data(first.password.utf8))
        for prompt in ["Enter passphrase for key:", "Continue connecting (yes/no)?", "Verification code:"] {
            precondition(SSHPassword.askpass(prompt:prompt, account:account, configuration:file) == nil)
        }
        for other in [SSHPassword.account(host:"other.invalid", user:"fixture", port:22),
                      SSHPassword.account(host:"fixture.invalid", user:"other", port:22),
                      SSHPassword.account(host:"fixture.invalid", user:"fixture", port:2222)] {
            precondition(SSHPassword.read(other, configuration:file) == nil)
        }
        let updated = SSHPassword.Connection(host:first.host, user:first.user, port:first.port, password:"changed fixture 密码")
        try write([updated]); precondition(SSHPassword.read(account, configuration:file) == Data(updated.password.utf8))
        try FileManager.default.setAttributes([.posixPermissions:0o644], ofItemAtPath:file.path)
        precondition(SSHPassword.read(account, configuration:file) == nil)
        try write([updated], version:2); precondition(SSHPassword.read(account, configuration:file) == nil)
        try write([first, first]); precondition(SSHPassword.read(account, configuration:file) == nil)
        try write([SSHPassword.Connection(host:first.host, user:first.user, port:first.port, password:"bad\nsecret")])
        precondition(SSHPassword.read(account, configuration:file) == nil)
        try write([SSHPassword.Connection(host:first.host, user:first.user, port:first.port, password:String(repeating:"a", count:4097))])
        precondition(SSHPassword.read(account, configuration:file) == nil)
        try Data("corrupt fixture".utf8).write(to:file)
        precondition(SSHPassword.read(account, configuration:file) == nil)
        try write([first]); let linked = root.appendingPathComponent("linked.json")
        try FileManager.default.createSymbolicLink(at:linked, withDestinationURL:file)
        precondition(SSHPassword.read(account, configuration:linked) == nil)
        print("PASS: private Mac service configuration, credential updates, endpoint isolation, permissions, corruption/version limits and password-only askpass; no Keychain access")
    }
}
