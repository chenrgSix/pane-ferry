import AppKit
@main struct MacPublisherBoundarySmoke {
    static func main() throws {
        precondition(receivedApplication(URL(fileURLWithPath:"/Applications/Firefox.app"),"io.lanremoteapp.host.fixture"))
        precondition(receivedApplication(URL(fileURLWithPath:"/Users/test/Library/Application Support/io.lanremoteapp.launcher/WindowHosts/generation/Firefox.app"),"org.mozilla.firefox"))
        precondition(receivedApplication(URL(fileURLWithPath:"/System/Library/CoreServices/SecurityAgent.app"),"com.apple.SecurityAgent"))
        precondition(!receivedApplication(URL(fileURLWithPath:"/Applications/Safari.app"),"com.apple.Safari"))
        precondition(dimensions(2560,1664));precondition(identifier("window-42"))
        for value in ["", "../Safari", "a;id",String(repeating:"a",count:101)] { precondition(!identifier(value)) }
        for (w,h) in [(0,1),(4096,4096),(4097,1),(-1,100)] { precondition(!dimensions(w,h)) }
        var data=Data([137,80,78,71,13,10,26,10,0,0,0,13,73,72,68,82,0,0,0,10,0,0,0,10])
        try checkPNG(data);data[16]=127
        do {try checkPNG(data);fatalError("oversized image accepted")} catch NodeError.clipboard_invalid {}
        print("PASS: Mac publisher exclusion, identifiers, frame and image budgets")
    }
}
