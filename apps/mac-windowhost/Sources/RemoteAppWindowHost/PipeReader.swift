import Darwin
import Foundation

/// FileHandle.read(upToCount:) may wait to fill its buffer on a pipe. A single
/// POSIX read returns as soon as SSH has any bytes, including a short hello.
func readPipeChunk(from handle: FileHandle, maximumCount: Int = 65_536) throws -> Data {
    precondition(maximumCount > 0)
    var data = Data(count: maximumCount)
    while true {
        let count = data.withUnsafeMutableBytes { bytes in
            Darwin.read(handle.fileDescriptor, bytes.baseAddress!, bytes.count)
        }
        if count >= 0 {
            data.count = count
            return data
        }
        if errno != EINTR { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
    }
}
