import Foundation
import Darwin
if let result = runCredentialCommand(Array(CommandLine.arguments.dropFirst())) { exit(result) }
var arguments = Array(CommandLine.arguments.dropFirst())
var target = "10.0.1.66"
if arguments.first == "--target" {
    guard arguments.count >= 3 else { exit(2) }
    target = arguments[1]; arguments.removeFirst(2)
    guard target.range(of:"^[0-9.]+$",options:.regularExpression) != nil else { exit(2) }
}
let process = Process(); process.executableURL = URL(fileURLWithPath:"/usr/bin/ssh")
var authentication = SSHAuthentication.configure(process,host:"10.0.1.66",user:"chenrg",port:22)
if let index = authentication.firstIndex(of:"-S") { authentication[index+1] = "/private/tmp/lra-path-diagnosis/ssh-" + target + ".sock" }
process.arguments = ["-F","/dev/null","-T","-oCompression=no","-vv","-E","/private/tmp/lra-path-diagnosis/ssh-"+target+".log"] + authentication + ["-oStrictHostKeyChecking=yes","-oHostKeyAlias=10.0.1.66","-oConnectTimeout=8","-oForwardAgent=no","-oClearAllForwardings=yes","-p","22","-l","chenrg","--",target] + arguments
try process.run(); process.waitUntilExit(); exit(process.terminationStatus)
