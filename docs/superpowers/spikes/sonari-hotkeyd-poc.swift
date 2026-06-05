// sonari-hotkeyd-poc.swift
// Minimal global-hotkey PoC for Sonari using Carbon RegisterEventHotKey.
// Registers ONE global hotkey (Control+Option+S) and prints a line each time
// it fires, EVEN while another app (e.g. a terminal) is frontmost.
//
// Why RegisterEventHotKey:
//   * Fires system-wide regardless of which app is frontmost.
//   * Consumes ONLY the exact registered combo; every other keystroke passes
//     through to the focused app untouched (no event tap, no swallowing typing).
//   * Does NOT require Accessibility or Input Monitoring permission (unlike
//     CGEventTap / NSEvent global monitors), because it is narrowly scoped.
//
// Build:  swiftc sonari-hotkeyd-poc.swift -o sonari-hotkeyd-poc
// Run:    ./sonari-hotkeyd-poc      (then press Ctrl+Opt+S in any app)
// Quit:   Ctrl+C
//
// Verified: compiles with zero errors and stays alive in the run loop on
// macOS 26.5.1 / Darwin 25.5.0 (arm64) using Apple's /usr/bin/swiftc.
//
// NOTE: the real hotkeyd default modifier is Ctrl+Cmd (Carbon controlKey|cmdKey),
// chosen to avoid VoiceOver's Ctrl+Opt. This PoC uses Ctrl+Opt only to mirror the
// spike; swap optionKey -> cmdKey for the shipping default.

import Carbon
import Cocoa

// 'SONI' four-char signature for our hotkey id.
let kHotKeySignature: OSType = 0x534F4E49 // 'S','O','N','I'

func fireTimestamp() -> String {
    let f = ISO8601DateFormatter()
    f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
    return f.string(from: Date())
}

// C-compatible callback invoked by the Carbon event loop when our hotkey fires.
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
        // In real hotkeyd: write the action name to the speech daemon's Unix socket.
        print("[\(fireTimestamp())] HOTKEY id=\(hkID.id) -> action=stop  (Ctrl+Opt+S)")
        fflush(stdout)
    }
    return noErr
}

// 1. Install the application-level event handler for hotkey-pressed events.
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
    FileHandle.standardError.write("InstallEventHandler failed: \(installStatus)\n".data(using: .utf8)!)
    exit(1)
}

// 2. Register Control+Option+S as a global hotkey.
//    kVK_ANSI_S = 1; modifiers use Carbon masks controlKey + optionKey.
var hotKeyRef: EventHotKeyRef?
let hotKeyID = EventHotKeyID(signature: kHotKeySignature, id: 1)
let modifiers = UInt32(controlKey | optionKey)
let regStatus = RegisterEventHotKey(
    UInt32(kVK_ANSI_S),
    modifiers,
    hotKeyID,
    GetApplicationEventTarget(),
    0,
    &hotKeyRef
)
guard regStatus == noErr, hotKeyRef != nil else {
    FileHandle.standardError.write("RegisterEventHotKey failed: \(regStatus)\n".data(using: .utf8)!)
    exit(1)
}

print("Sonari hotkeyd PoC ready. Press Ctrl+Opt+S in ANY app. Ctrl+C to quit.")
fflush(stdout)

// 3. Drive the Carbon event loop. A LaunchAgent (Aqua session) gives us the
//    active GUI session this requires. NSApplication.run provides the run loop;
//    we keep it headless (no Dock icon) via .accessory activation policy.
let app = NSApplication.shared
app.setActivationPolicy(.accessory)
app.run()
