import Foundation

// No XCTest dependency: also runs on Macs with only Command Line Tools.
@main struct PipeReaderSmoke {
    static func main() throws {
        let pipe = Pipe()
        let hello = Data("{\"type\":\"hello\",\"version\":1}\n".utf8)
        try pipe.fileHandleForWriting.write(contentsOf: hello)
        let completed = DispatchSemaphore(value: 0)
        DispatchQueue.global().async {
            defer { completed.signal() }
            precondition(try! readPipeChunk(from: pipe.fileHandleForReading) == hello)
        }
        let arrived = completed.wait(timeout: .now() + 1)
        // Unblock even a broken reader, so the regression can fail promptly.
        try pipe.fileHandleForWriting.close()
        if arrived == .timedOut { _ = completed.wait(timeout: .now() + 2) }
        try pipe.fileHandleForReading.close()
        precondition(arrived == .success, "A short bridge hello must not wait for EOF or 64 KiB")

        let bounded = Pipe()
        try bounded.fileHandleForWriting.write(contentsOf: Data("abcdef".utf8))
        try bounded.fileHandleForWriting.close()
        let first = try readPipeChunk(from: bounded.fileHandleForReading, maximumCount: 3)
        let second = try readPipeChunk(from: bounded.fileHandleForReading, maximumCount: 3)
        let eof = try readPipeChunk(from: bounded.fileHandleForReading)
        precondition(first == Data("abc".utf8) && second == Data("def".utf8) && eof.isEmpty)
        try bounded.fileHandleForReading.close()
        print("PASS: short bridge handshake, bounded chunks, EOF")
    }
}
