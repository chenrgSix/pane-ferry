import AppKit

// Explicit operations only. Reverse-direction rich transfers are bounded to
// 8 MiB/256 entries; no archive extraction, arbitrary paths or background sync.
let reverseClipboardLimit=8*1024*1024
func readClipboard() throws -> [String:Any] {
    let board=NSPasteboard.general
    if let urls=board.readObjects(forClasses:[NSURL.self],options:[.urlReadingFileURLsOnly:true]) as? [URL],!urls.isEmpty {
        var entries=[[String:Any]](),total=0
        func visit(_ url:URL,_ name:String) throws {
            guard entries.count<256 else { throw NodeError.clipboard_invalid }
            let values=try url.resourceValues(forKeys:[.isRegularFileKey,.isDirectoryKey,.isSymbolicLinkKey,.fileSizeKey])
            guard values.isSymbolicLink != true else { throw NodeError.clipboard_invalid }
            if values.isDirectory == true {
                entries.append(["path":name,"directory":true,"size":0])
                for child in try FileManager.default.contentsOfDirectory(at:url,includingPropertiesForKeys:nil).sorted(by:{$0.path<$1.path}) {
                    try visit(child,name+"/"+child.lastPathComponent)
                }
            } else {
                guard values.isRegularFile == true,let size=values.fileSize,size<=reverseClipboardLimit-total else { throw NodeError.clipboard_invalid }
                let data=try Data(contentsOf:url);total+=data.count
                guard data.count==size,total<=reverseClipboardLimit else { throw NodeError.clipboard_invalid }
                entries.append(["path":name,"directory":false,"size":data.count,"data":data.base64EncodedString()])
            }
        }
        for url in urls { try visit(url,url.lastPathComponent) }
        return ["kind":"files","entries":entries]
    }
    var png=board.data(forType:.png)
    if png==nil,let tiff=board.data(forType:.tiff),tiff.count<=reverseClipboardLimit,let image=NSBitmapImageRep(data:tiff),image.pixelsWide*image.pixelsHigh<=pixelLimit {
        png=image.representation(using:.png,properties:[:])
    }
    if let png=png,png.count<=reverseClipboardLimit {
        try checkPNG(png)
        return ["kind":"image","entries":[["path":"image.png","directory":false,"size":png.count,"data":png.base64EncodedString()]]]
    }
    if let text=board.string(forType:.string),!text.isEmpty,text.utf8.count<=8192,!text.contains("\0") { return ["kind":"text","text":text] }
    throw NodeError.clipboard_invalid
}
func checkPNG(_ data:Data) throws {
    guard data.count>=24,data.count<=reverseClipboardLimit,data.prefix(8)==Data([137,80,78,71,13,10,26,10]),
        data[12..<16]==Data("IHDR".utf8) else { throw NodeError.clipboard_invalid }
    let width=data[16..<20].reduce(0){($0<<8)|Int($1)},height=data[20..<24].reduce(0){($0<<8)|Int($1)}
    guard width>0,height>0,width<=32768,height<=32768,width*height<=pixelLimit else { throw NodeError.clipboard_invalid }
}
// Only UUID directories created here are collectible. Preserve all roots
// referenced by the live pasteboard, and never follow symbolic links.
func prepareClipboardCache(_ root: URL, incoming: Int, protecting: Set<String>, now: Date = Date()) throws {
    try privateDirectory(root)
    var entries=[(URL,Date,Int)](), total=0
    for child in try FileManager.default.contentsOfDirectory(at:root,includingPropertiesForKeys:[.isDirectoryKey,.isSymbolicLinkKey,.contentModificationDateKey]) {
        guard UUID(uuidString:child.lastPathComponent) != nil else { continue }
        let info=try child.resourceValues(forKeys:[.isDirectoryKey,.isSymbolicLinkKey,.contentModificationDateKey])
        guard info.isDirectory == true, info.isSymbolicLink != true else { continue }
        var bytes=0
        if let walk=FileManager.default.enumerator(at:child,includingPropertiesForKeys:[.isRegularFileKey,.isSymbolicLinkKey,.fileSizeKey]) {
            for case let file as URL in walk {
                let value=try file.resourceValues(forKeys:[.isRegularFileKey,.isSymbolicLinkKey,.fileSizeKey])
                if value.isSymbolicLink == true { walk.skipDescendants(); continue }
                if value.isRegularFile == true { bytes += value.fileSize ?? 0 }
            }
        }
        total += bytes; entries.append((child,info.contentModificationDate ?? .distantPast,bytes))
    }
    var count=entries.count
    for (url,date,bytes) in entries.sorted(by:{$0.1<$1.1}) where !protecting.contains(url.lastPathComponent) {
        if now.timeIntervalSince(date)>7*86400 || total+incoming>4*reverseClipboardLimit || count>=128 {
            try FileManager.default.removeItem(at:url);total-=bytes;count-=1
        }
    }
    guard total+incoming<=4*reverseClipboardLimit, count<128 else { throw NodeError.clipboard_invalid }
}
func writeClipboard(_ body:[String:Any],directory:URL) throws {
    let board=NSPasteboard.general
    if body["kind"] as? String == "text" {
        guard let text=body["text"] as? String,!text.isEmpty,text.utf8.count<=8192,!text.contains("\0") else { throw NodeError.clipboard_invalid }
        board.clearContents();board.setString(text,forType:.string);return
    }
    guard let kind=body["kind"] as? String,["image","files"].contains(kind),let entries=body["entries"] as? [[String:Any]],!entries.isEmpty,entries.count<=256 else { throw NodeError.clipboard_invalid }
    var names=Set<String>(),directories=Set<String>(),total=0,validated=[(String,Bool,Data)]()
    for entry in entries {
        guard let path=entry["path"] as? String,path.utf8.count<=1024,!path.contains("\\"),!path.unicodeScalars.contains(where:{$0.value<32}),
            !path.split(separator:"/",omittingEmptySubsequences:false).contains(where:{$0.isEmpty || $0=="." || $0==".."}),
            names.insert(path.folding(options:.caseInsensitive,locale:Locale(identifier:"en_US_POSIX"))).inserted,
            let dir=entry["directory"] as? Bool,let size=entry["size"] as? Int,size>=0,size<=reverseClipboardLimit-total else { throw NodeError.clipboard_invalid }
        let parent=(path as NSString).deletingLastPathComponent
        guard parent.isEmpty || directories.contains(parent) else { throw NodeError.clipboard_invalid }
        let data:Data
        if dir { guard size==0 else { throw NodeError.clipboard_invalid };directories.insert(path);data=Data() }
        else { guard let encoded=entry["data"] as? String,encoded.utf8.count<=12*1024*1024,let bytes=Data(base64Encoded:encoded),bytes.count==size else { throw NodeError.clipboard_invalid };data=bytes }
        total+=size;validated.append((path,dir,data))
    }
    if kind=="image" {
        guard validated.count==1,!validated[0].1 else { throw NodeError.clipboard_invalid }
        try checkPNG(validated[0].2)
        board.clearContents();let item=NSPasteboardItem();item.setData(validated[0].2,forType:.png)
        if let image=NSBitmapImageRep(data:validated[0].2),let tiff=image.tiffRepresentation { item.setData(tiff,forType:.tiff) }
        guard board.writeObjects([item]) else { throw NodeError.clipboard_invalid };return
    }
    let cache=directory.appendingPathComponent("Clipboard")
    let current=board.readObjects(forClasses:[NSURL.self],options:[.urlReadingFileURLsOnly:true]) as? [URL] ?? []
    let pins=Set(current.filter{$0.path.hasPrefix(cache.path+"/")}.compactMap{$0.path.dropFirst(cache.path.count+1).split(separator:"/").first.map(String.init)})
    try prepareClipboardCache(cache,incoming:total,protecting:pins)
    let root=cache.appendingPathComponent(UUID().uuidString)
    try privateDirectory(root)
    var complete=false
    defer { if !complete { try? FileManager.default.removeItem(at:root) } }
    for (name,dir,data) in validated {
        let path=root.appendingPathComponent(name)
        if dir { try privateDirectory(path) }
        else { guard FileManager.default.createFile(atPath:path.path,contents:data,attributes:[.posixPermissions:0o600]) else { throw NodeError.clipboard_invalid } }
    }
    let roots=validated.filter{!$0.0.contains("/")}.map{root.appendingPathComponent($0.0) as NSURL}
    board.clearContents();guard board.writeObjects(roots) else { throw NodeError.clipboard_invalid }
    complete=true
}
