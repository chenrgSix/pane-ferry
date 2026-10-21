import AppKit

let controllerTextLimit = 8 * 1024

final class SurfaceView: NSView, NSTextInputClient {
    var remoteID: UInt32 = 0
    var remoteWidth = 1
    var remoteHeight = 1
    var image: CGImage? { didSet { needsDisplay = true } }
    var preservesAspectRatio = false { didSet { needsDisplay = true } }
    var transmit: (([String: Any]) -> Void)?
    var controllerTextEnabled = true
    var controllerPasteEnabled = true
    var controllerCopyEnabled = true
    var readControllerClipboard: () -> String? = { NSPasteboard.general.string(forType: .string) }
    private var markedText = NSAttributedString(string: "")
    private var markedSelection = NSRange(location: 0, length: 0)
    private var commandEvent: NSEvent?
    private var insertionPoint = NSPoint(x: 20, y: 20)
    private var keys: [UInt16: UInt32] = [:]
    private var heldModifiers: Set<UInt32> = []
    private var heldButtons: Set<Int> = []
    private var lastPointer = (x: 0, y: 0)
    private var tracking: NSTrackingArea?
    private var imageRect: CGRect {
        preservesAspectRatio ? RenderGeometry.imageRect(in: bounds, width: remoteWidth, height: remoteHeight) : bounds
    }
    override var isFlipped: Bool { true }
    override var acceptsFirstResponder: Bool { true }
    override func draw(_ dirtyRect: NSRect) {
        NSColor.black.setFill(); bounds.fill()
        guard let image = image else { return }
        NSImage(cgImage: image, size: NSSize(width: CGFloat(image.width), height: CGFloat(image.height)))
            .draw(in: imageRect, from: .zero, operation: .copy, fraction: 1, respectFlipped: true, hints: [.interpolation: NSImageInterpolation.high])
        if hasMarkedText() {
            let preview = NSAttributedString(string: markedText.string, attributes: [
                .font: NSFont.systemFont(ofSize: 16), .foregroundColor: NSColor.labelColor,
                .backgroundColor: NSColor.textBackgroundColor, .underlineStyle: NSUnderlineStyle.single.rawValue])
            preview.draw(at: insertionPoint)
        }
    }
    override func updateTrackingAreas() {
        if let tracking = tracking { removeTrackingArea(tracking) }
        tracking = NSTrackingArea(rect: .zero, options: [.activeInKeyWindow, .mouseMoved, .inVisibleRect], owner: self, userInfo: nil)
        addTrackingArea(tracking!); super.updateTrackingAreas()
    }
    private func pointer(_ event: NSEvent, button: Int, pressed: Bool) {
        let point = convert(event.locationInWindow, from: nil)
        let rect = imageRect
        if !rect.contains(point) && (button == 0 ? heldButtons.isEmpty : !heldButtons.contains(button)) { return }
        let x = max(0, min(remoteWidth - 1, Int((point.x - rect.minX) / max(rect.width, 1) * CGFloat(remoteWidth))))
        let y = max(0, min(remoteHeight - 1, Int((point.y - rect.minY) / max(rect.height, 1) * CGFloat(remoteHeight))))
        lastPointer = (x, y)
        if button > 0 { if pressed { heldButtons.insert(button) } else { heldButtons.remove(button) } }
        transmit?(["type": "input.pointer", "window_id": remoteID, "x": x, "y": y, "button": button, "pressed": pressed])
    }
    override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        insertionPoint = convert(event.locationInWindow, from: nil)
        inputContext?.discardMarkedText(); unmarkText()
        pointer(event, button: 1, pressed: true)
    }
    override func mouseUp(with event: NSEvent) { pointer(event, button: 1, pressed: false) }
    override func rightMouseDown(with event: NSEvent) { pointer(event, button: 3, pressed: true) }
    override func rightMouseUp(with event: NSEvent) { pointer(event, button: 3, pressed: false) }
    override func otherMouseDown(with event: NSEvent) { pointer(event, button: 2, pressed: true) }
    override func otherMouseUp(with event: NSEvent) { pointer(event, button: 2, pressed: false) }
    override func mouseMoved(with event: NSEvent) { pointer(event, button: 0, pressed: false) }
    override func mouseDragged(with event: NSEvent) { pointer(event, button: 0, pressed: false) }
    override func rightMouseDragged(with event: NSEvent) { pointer(event, button: 0, pressed: false) }
    override func otherMouseDragged(with event: NSEvent) { pointer(event, button: 0, pressed: false) }
    override func scrollWheel(with event: NSEvent) {
        let vertical = event.scrollingDeltaY
        let horizontal = event.scrollingDeltaX
        let value = abs(vertical) >= abs(horizontal) ? vertical : horizontal
        guard value != 0 else { return }
        let button = abs(vertical) >= abs(horizontal) ? (value > 0 ? 4 : 5) : (value > 0 ? 6 : 7)
        for _ in 0..<min(5, max(1, Int(abs(value) / (event.hasPreciseScrollingDeltas ? 10 : 1)))) {
            pointer(event, button: button, pressed: true); pointer(event, button: button, pressed: false)
        }
    }
    private func key(_ symbol: UInt32, _ down: Bool) { transmit?(["type": "input.key", "window_id": remoteID, "keysym": symbol, "pressed": down]) }
    override func keyDown(with event: NSEvent) {
        let shortcut = event.modifierFlags.intersection([.command, .control])
        if !shortcut.isEmpty {
            // AppKit can swallow keyUp while Command is held. Complete each
            // Command shortcut here; local repeat events supply further strokes.
            let command = event.modifierFlags.contains(.command)
            if command { releaseKeyboard() }
            defer { if command { releaseKeyboard() } }
            if event.keyCode == 9 { paste(nil); return }
            if event.modifierFlags.contains(.command) && event.keyCode == 8 { copy(nil); return }
            if event.modifierFlags.contains(.command) && event.keyCode == 7 { NSSound.beep(); return }
            rawKeyDown(event); return
        }
        commandEvent = event
        interpretKeyEvents([event])
        commandEvent = nil
    }
    override func keyUp(with event: NSEvent) {
        if let symbol = keys.removeValue(forKey: event.keyCode) { key(symbol, false) }
        if keys.isEmpty { releaseKeyboard() } else { syncModifiers(event.modifierFlags) }
    }
    override func flagsChanged(with event: NSEvent) {
        // Modifiers used by the controller IME must never toggle the remote IME.
        if !keys.isEmpty { syncModifiers(event.modifierFlags) }
    }
    private func rawKeyDown(_ event: NSEvent) {
        syncModifiers(event.modifierFlags)
        guard let symbol = keys[event.keyCode] ?? Self.keysym(event) else { NSSound.beep(); return }
        keys[event.keyCode] = symbol; key(symbol, true)
    }
    override func doCommand(by selector: Selector) {
        if let event = commandEvent { rawKeyDown(event) }
    }
    func insertText(_ string: Any, replacementRange: NSRange) {
        let text = (string as? NSAttributedString)?.string ?? (string as? String ?? "")
        unmarkText(); releaseKeyboard()
        guard controllerTextEnabled, !text.isEmpty, text.utf8.count <= controllerTextLimit, !text.contains("\0") else { NSSound.beep(); return }
        transmit?(["type": "input.text.commit", "window_id": remoteID, "text": text])
    }
    func setMarkedText(_ string: Any, selectedRange: NSRange, replacementRange: NSRange) {
        let value = (string as? NSAttributedString) ?? NSAttributedString(string: string as? String ?? "")
        guard value.string.utf8.count <= controllerTextLimit else { NSSound.beep(); return }
        markedText = value; markedSelection = selectedRange; needsDisplay = true
        inputContext?.invalidateCharacterCoordinates()
    }
    func unmarkText() { markedText = NSAttributedString(string: ""); markedSelection = NSRange(location: 0, length: 0); needsDisplay = true }
    func hasMarkedText() -> Bool { markedText.length > 0 }
    func markedRange() -> NSRange { hasMarkedText() ? NSRange(location: 0, length: markedText.length) : NSRange(location: NSNotFound, length: 0) }
    func selectedRange() -> NSRange { markedSelection }
    func validAttributesForMarkedText() -> [NSAttributedString.Key] { [] }
    func attributedSubstring(forProposedRange range: NSRange, actualRange: NSRangePointer?) -> NSAttributedString? {
        let valid = NSIntersectionRange(range, NSRange(location: 0, length: markedText.length))
        guard valid.length > 0 else { return nil }
        actualRange?.pointee = valid; return markedText.attributedSubstring(from: valid)
    }
    func characterIndex(for point: NSPoint) -> Int { NSNotFound }
    func firstRect(forCharacterRange range: NSRange, actualRange: NSRangePointer?) -> NSRect {
        actualRange?.pointee = markedRange()
        let rect = convert(NSRect(x: insertionPoint.x, y: insertionPoint.y, width: 1, height: 22), to: nil)
        return window?.convertToScreen(rect) ?? .zero
    }
    @objc func paste(_ sender: Any?) {
        guard controllerPasteEnabled, let text = readControllerClipboard(), !text.isEmpty,
              text.utf8.count <= controllerTextLimit, !text.contains("\0") else { NSSound.beep(); return }
        inputContext?.discardMarkedText(); unmarkText(); releaseKeyboard()
        transmit?(["type": "clipboard.paste", "window_id": remoteID, "text": text])
    }
    @objc func copy(_ sender: Any?) {
        guard controllerCopyEnabled else { NSSound.beep(); return }
        releaseKeyboard()
        transmit?(["type": "clipboard.copy", "window_id": remoteID, "request_id": UUID().uuidString])
    }
    private func syncModifiers(_ flags: NSEvent.ModifierFlags) {
        var target = Set<UInt32>()
        if flags.contains(.shift) { target.insert(0xffe1) }
        if flags.contains(.control) || flags.contains(.command) { target.insert(0xffe3) }
        if flags.contains(.option) { target.insert(0xffe9) }
        for symbol in heldModifiers.subtracting(target) { key(symbol, false) }
        for symbol in target.subtracting(heldModifiers) { key(symbol, true) }
        heldModifiers = target
    }
    func releaseInput() {
        for button in heldButtons { transmit?(["type": "input.pointer", "window_id": remoteID, "x": lastPointer.x, "y": lastPointer.y, "button": button, "pressed": false]) }
        heldButtons.removeAll()
        releaseKeyboard()
        inputContext?.discardMarkedText(); unmarkText()
    }
    private func releaseKeyboard() {
        for symbol in Set(keys.values) { key(symbol, false) }; keys.removeAll()
        for symbol in heldModifiers { key(symbol, false) }; heldModifiers.removeAll()
    }
    static func keysym(_ event: NSEvent) -> UInt32? {
        let special: [UInt16: UInt32] = [36: 0xff0d, 48: 0xff09, 51: 0xff08, 53: 0xff1b, 117: 0xffff,
            123: 0xff51, 124: 0xff53, 125: 0xff54, 126: 0xff52, 115: 0xff50, 119: 0xff57,
            116: 0xff55, 121: 0xff56, 76: 0xff8d,
            122: 0xffbe, 120: 0xffbf, 99: 0xffc0, 118: 0xffc1, 96: 0xffc2, 97: 0xffc3,
            98: 0xffc4, 100: 0xffc5, 101: 0xffc6, 109: 0xffc7, 103: 0xffc8, 111: 0xffc9]
        if let symbol = special[event.keyCode] { return symbol }
        // Physical symbols are only used for shortcuts; ordinary text uses the controller IME.
        let ansi: [UInt16: String] = [0:"a",1:"s",2:"d",3:"f",4:"h",5:"g",6:"z",7:"x",8:"c",9:"v",11:"b",
            12:"q",13:"w",14:"e",15:"r",16:"y",17:"t",18:"1",19:"2",20:"3",21:"4",22:"6",23:"5",24:"=",25:"9",26:"7",27:"-",28:"8",29:"0",
            30:"]",31:"o",32:"u",33:"[",34:"i",35:"p",37:"l",38:"j",39:"'",40:"k",41:";",42:"\\",43:",",44:"/",45:"n",46:"m",47:".",49:" ",50:"`"]
        return ansi[event.keyCode]?.unicodeScalars.first?.value
    }
}
