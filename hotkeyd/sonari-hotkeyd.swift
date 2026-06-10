// sonari-hotkeyd.swift
// Sonari Phase 2 global-hotkey daemon.
//
// Reads ~/.sonari/hotkeyd.resolved.json — an array of
//   { "action": String, "keyCode": Int, "modifiers": Int, "message": String }
// produced by sonari.keymap.write_resolved(). For each entry it registers a
// Carbon global hotkey (RegisterEventHotKey: fires system-wide, consumes only
// the registered combo, needs NO macOS permission). On fire it reads the
// daemon's port + token from ~/.sonari/daemon.lock and writes the session
// token plus the entry's `message` (each newline-terminated) to the speechd
// localhost-TCP listener at 127.0.0.1 (best-effort; errors ignored).
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

// Read ~/.sonari/daemon.lock for the daemon's ephemeral port + session token.
func lockInfo() -> (port: UInt16, token: String)? {
    let path = (sonariDir() as NSString).appendingPathComponent("daemon.lock")
    guard let data = FileManager.default.contents(atPath: path),
          let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
          let port = obj["port"] as? Int,
          let token = obj["token"] as? String else { return nil }
    return (UInt16(port), token)
}

// Best-effort: connect to the speechd localhost-TCP listener and write
// the session token followed by one newline-JSON message line.
func sendMessage(_ message: String) {
    guard let info = lockInfo() else { return }
    let fd = socket(AF_INET, SOCK_STREAM, 0)
    if fd < 0 { return }
    defer { close(fd) }
    var addr = sockaddr_in()
    addr.sin_family = sa_family_t(AF_INET)
    addr.sin_port = info.port.bigEndian
    inet_pton(AF_INET, "127.0.0.1", &addr.sin_addr)
    let connected = withUnsafePointer(to: &addr) { aptr -> Int32 in
        aptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sptr in
            connect(fd, sptr, socklen_t(MemoryLayout<sockaddr_in>.size))
        }
    }
    if connected != 0 { return }
    let line = info.token + "\n" + message + "\n"
    _ = line.withCString { write(fd, $0, strlen($0)) }
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

// 3. Run the Carbon event loop headlessly (no Dock icon).
let app = NSApplication.shared
app.setActivationPolicy(.accessory)
app.run()
