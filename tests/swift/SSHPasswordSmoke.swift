import Foundation
import Security

@main
struct SSHPasswordSmoke {
    static func main() throws {
        let root = URL(fileURLWithPath: CommandLine.arguments[1])
        precondition(!SSHAuthentication.trustedAskpassCaller(), "a direct CLI invocation cannot export credentials")
        let path = root.appendingPathComponent("fixture.keychain-db").path
        var keychain: SecKeychain?
        let keychainPassword = Array("private-test-keychain".utf8)
        let status = keychainPassword.withUnsafeBytes { bytes in
            SecKeychainCreate(path, UInt32(bytes.count), bytes.baseAddress, false, nil, &keychain)
        }
        precondition(status == errSecSuccess, "isolated test Keychain could not be created: \(status)")
        guard let keychain = keychain else { fatalError("missing private test Keychain") }
        defer { SecKeychainDelete(keychain) }
        let account = SSHPassword.account(host:"fixture.invalid", user:"fixture", port:22)
        let secret = Data("fixture-password-value".utf8)
        precondition(!SSHPassword.contains(account, keychain:keychain))
        precondition(SSHPassword.save(secret, account:account, keychain:keychain))
        precondition(SSHPassword.contains(account, keychain:keychain))
        precondition(SSHPassword.read(account, keychain:keychain) == secret)
        precondition(SSHPassword.askpass(prompt:"fixture@fixture.invalid's password: ", account:account, keychain:keychain) == secret)
        for prompt in ["Enter passphrase for key:", "Are you sure you want to continue connecting (yes/no)?", "Verification code:"] {
            precondition(SSHPassword.askpass(prompt:prompt, account:account, keychain:keychain) == nil)
        }
        for other in [SSHPassword.account(host:"other.invalid", user:"fixture", port:22),
                      SSHPassword.account(host:"fixture.invalid", user:"other", port:22),
                      SSHPassword.account(host:"fixture.invalid", user:"fixture", port:2222)] {
            precondition(SSHPassword.read(other, keychain:keychain) == nil)
        }
        let updated = Data("updated-fixture-password".utf8)
        precondition(SSHPassword.save(updated, account:account, keychain:keychain))
        precondition(SSHPassword.read(account, keychain:keychain) == updated)
        precondition(!SSHPassword.save(Data("invalid\nsecret".utf8), account:account, keychain:keychain))
        precondition(SSHPassword.read(account, keychain:keychain) == updated)
        print("PASS: private Keychain password storage/update, endpoint isolation, bounded secrets and password-only askpass; system credentials unchanged")
    }
}
