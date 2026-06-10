// sonari-hotkeyd.swift
// Sonari Phase 2 global-hotkey daemon.
//
// Reads ~/.sonari/hotkeyd.resolved.json — an array of
//   { "action": String, "keyCode": Int, "modifiers": Int, "message": String }
// produced by sonari.keymap.write_resolved(). For each entry it registers a
// Carbon global hotkey (RegisterEventHotKey: fires system-wide, consumes only
// the registered combo, needs NO macOS permission). On fire it writes the
// entry's `message` plus a newline to the speechd Unix socket at
// ~/.sonari/speechd.sock (best-effort; errors ignored).
//
// Build: swiftc hotkeyd/sonari-hotkeyd.swift -o ~/.sonari/sonari-hotkeyd
// Run:   the com.sonari.hotkeyd LaunchAgent (Aqua session, .accessory policy).

import Carbon
import Cocoa

let kHotKeySignature: OSType = 0x534F4E49  // 'SONI'

struct HotkeyEntry {
    let keyCode: UInt32
    let modifiers: UInt32
    let message: String
}

func sonariDir() -> String {
    return (NSHomeDirectory() as NSString).appendingPathComponent(".sonari")
}

func resolvedPath() -> String {
    return (sonariDir() as NSString).appendingPathComponent("hotkeyd.resolved.json")
}

func socketPath() -> String {
    return (sonariDir() as NSString).appendingPathComponent("speechd.sock")
}

// Parse the resolved JSON array into HotkeyEntry values.
func loadEntries() -> [HotkeyEntry] {
    guard let data = FileManager.default.contents(atPath: resolvedPath()) else {
        FileHandle.standardError.write(
            "hotkeyd: cannot read \(resolvedPath())\n".data(using: .utf8)!)
        return []
    }
    guard let parsed = try? JSONSerialization.jsonObject(with: data),
          let array = parsed as? [[String: Any]] else {
        FileHandle.standardError.write("hotkeyd: malformed resolved JSON\n".data(using: .utf8)!)
        return []
    }
    var entries: [HotkeyEntry] = []
    for obj in array {
        guard let keyCode = obj["keyCode"] as? Int,
              let modifiers = obj["modifiers"] as? Int,
              let message = obj["message"] as? String else {
            continue
        }
        entries.append(HotkeyEntry(
            keyCode: UInt32(keyCode),
            modifiers: UInt32(modifiers),
            message: message))
    }
    return entries
}

// Best-effort: connect to the speechd Unix socket and write one newline-JSON line.
func sendMessage(_ message: String) {
    let fd = socket(AF_UNIX, SOCK_STREAM, 0)
    if fd < 0 { return }
    defer { close(fd) }

    var addr = sockaddr_un()
    addr.sun_family = sa_family_t(AF_UNIX)
    let path = socketPath()
    let maxLen = MemoryLayout.size(ofValue: addr.sun_path)
    _ = path.withCString { cstr in
        withUnsafeMutablePointer(to: &addr.sun_path) { ptr in
            ptr.withMemoryRebound(to: CChar.self, capacity: maxLen) { dst in
                strncpy(dst, cstr, maxLen - 1)
            }
        }
    }
    let size = socklen_t(MemoryLayout<sockaddr_un>.size)
    let connected = withUnsafePointer(to: &addr) { aptr -> Int32 in
        aptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sptr in
            connect(fd, sptr, size)
        }
    }
    if connected != 0 { return }

    let line = message + "\n"
    _ = line.withCString { cstr in
        write(fd, cstr, strlen(cstr))
    }
}

// Index entries by their hotkey id so the handler can look up the message.
var entriesByID: [UInt32: HotkeyEntry] = [:]

let hotKeyHandler: EventHandlerUPP = { (_ nextHandler, _ theEvent, _ userData) -> OSStatus in
    var hkID = EventHotKeyID()
    let status = GetEventParameter(
        theEvent,
        EventParamName(kEventParamDirectObject),
        EventParamType(typeEventHotKeyID),
        nil,
        MemoryLayout<EventHotKeyID>.size,
        nil,
        &hkID
    )
    if status == noErr && hkID.signature == kHotKeySignature {
        if let entry = entriesByID[hkID.id] {
            sendMessage(entry.message)
        }
    }
    return noErr
}

// Phase 2.1: doctor probe. Exit 0 iff Input Monitoring is granted (the
// listen-only arrow tap needs it; everything else works without it).
if CommandLine.arguments.contains("--check-input-monitoring") {
    exit(CGPreflightListenEventAccess() ? 0 : 1)
}

// 1. Install the keyboard event handler for hotkey-pressed events.
var eventType = EventTypeSpec(
    eventClass: OSType(kEventClassKeyboard),
    eventKind: UInt32(kEventHotKeyPressed)
)
let installStatus = InstallEventHandler(
    GetApplicationEventTarget(),
    hotKeyHandler,
    1,
    &eventType,
    nil,
    nil
)
guard installStatus == noErr else {
    FileHandle.standardError.write(
        "hotkeyd: InstallEventHandler failed: \(installStatus)\n".data(using: .utf8)!)
    exit(1)
}

// 2. Register each resolved entry. Keep the refs alive for the process lifetime.
var hotKeyRefs: [EventHotKeyRef?] = []
let entries = loadEntries()
for (index, entry) in entries.enumerated() {
    let id = UInt32(index)
    entriesByID[id] = entry
    var ref: EventHotKeyRef?
    let hotKeyID = EventHotKeyID(signature: kHotKeySignature, id: id)
    let regStatus = RegisterEventHotKey(
        entry.keyCode,
        entry.modifiers,
        hotKeyID,
        GetApplicationEventTarget(),
        0,
        &ref
    )
    if regStatus != noErr {
        // A claimed combo: log and continue with the rest.
        FileHandle.standardError.write(
            "hotkeyd: RegisterEventHotKey failed for id \(id) (status \(regStatus))\n"
                .data(using: .utf8)!)
        continue
    }
    hotKeyRefs.append(ref)
}

FileHandle.standardError.write(
    "hotkeyd: registered \(hotKeyRefs.count)/\(entries.count) hotkeys\n".data(using: .utf8)!)

// Phase 2.1: caret tracking. A LISTEN-ONLY CGEventTap observes arrow-key
// keyDown events (codes 125 down / 126 up) and forwards a caret_move message
// to speechd — but ONLY while ~/.sonari/prompt-open exists (the daemon
// creates/removes that flag around caret-trackable prompts). Listen-only
// means the event is NEVER consumed: the Claude Code TUI still receives
// every arrow press, so the mirror and the real highlight move together.
// Requires the Input Monitoring permission; without it the tap is skipped
// and Sonari runs exactly as before (hotkeys are Carbon, unaffected).
func promptOpenPath() -> String {
    return (sonariDir() as NSString).appendingPathComponent("prompt-open")
}

let arrowTapCallback: CGEventTapCallBack = { _, type, event, _ in
    if type == .keyDown {
        let code = event.getIntegerValueField(.keyboardEventKeycode)
        if (code == 125 || code == 126)
            && FileManager.default.fileExists(atPath: promptOpenPath()) {
            let dir = (code == 125) ? "down" : "up"
            sendMessage("{\"type\": \"caret_move\", \"dir\": \"\(dir)\"}")
        }
    }
    return Unmanaged.passUnretained(event)
}

if CGPreflightListenEventAccess() {
    let mask = CGEventMask(1 << CGEventType.keyDown.rawValue)
    if let tap = CGEvent.tapCreate(
        tap: .cgSessionEventTap,
        place: .headInsertEventTap,
        options: .listenOnly,
        eventsOfInterest: mask,
        callback: arrowTapCallback,
        userInfo: nil
    ) {
        let source = CFMachPortCreateRunLoopSource(kCFAllocatorDefault, tap, 0)
        CFRunLoopAddSource(CFRunLoopGetCurrent(), source, .commonModes)
        CGEvent.tapEnable(tap: tap, enable: true)
        FileHandle.standardError.write(
            "hotkeyd: caret tap installed (listen-only)\n".data(using: .utf8)!)
    }
} else {
    // Ask for the permission ONCE per machine (the system shows its own
    // dialog and lists "sonari-hotkeyd" under Input Monitoring). A marker
    // file prevents re-prompting on every login.
    let marker = (sonariDir() as NSString)
        .appendingPathComponent(".input-monitoring-requested")
    if !FileManager.default.fileExists(atPath: marker) {
        FileManager.default.createFile(atPath: marker, contents: nil)
        _ = CGRequestListenEventAccess()
    }
    FileHandle.standardError.write(
        "hotkeyd: caret tracking disabled (Input Monitoring not granted)\n"
            .data(using: .utf8)!)
}

// 3. Run the Carbon event loop headlessly (no Dock icon).
let app = NSApplication.shared
app.setActivationPolicy(.accessory)
app.run()
