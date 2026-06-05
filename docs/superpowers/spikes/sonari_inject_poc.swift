// sonari_inject_poc.swift
// Sonari injection proof-of-concept (macOS).  *** REFERENCE ONLY — DEFERRED ***
// Phase 2 does NOT build key injection (native numeric selection makes it
// unnecessary, and Secure Event Input silently swallows injected keys). This is
// kept for a possible FUTURE "read/act on text selection" feature, which is the
// only thing that would need Accessibility.
//
// BUILD (no third-party deps; uses the swiftc that ships with Xcode CLT):
//     swiftc sonari_inject_poc.swift -o sonari_inject_poc
//
// FIRST RUN: it will likely fail the Accessibility check and open the
// Accessibility pane. Add the built `sonari_inject_poc` binary there
// (or, if you run it straight from Terminal/iTerm, grant Accessibility to
// that terminal). Then run it again.
//
// RUN:
//     ./sonari_inject_poc
//   Then within 3 seconds, click/focus the terminal window running your
//   TUI picker. It will inject: Down, Down, Enter into the focused app.
//
// IT WILL REFUSE TO INJECT (and tell you why) if:
//   - it is not a trusted Accessibility client, OR
//   - Secure Event Input is enabled (terminal Secure Keyboard Entry or a
//     password field) — synthetic keys are silently swallowed in that case.
//
// This mirrors exactly the guards Sonari's real helper should enforce.

import Foundation
import CoreGraphics
import AppKit
import Carbon.HIToolbox        // IsSecureEventInputEnabled()
import ApplicationServices     // AXIsProcessTrusted / WithOptions

// macOS virtual key codes
let VK_DOWN: CGKeyCode  = 125
let VK_UP: CGKeyCode    = 126
let VK_RETURN: CGKeyCode = 36
let VK_TAB: CGKeyCode   = 48
let VK_SPACE: CGKeyCode = 49

// Terminals we consider safe to inject into.
let ALLOWED_BUNDLE_IDS: Set<String> = [
    "com.apple.Terminal",
    "com.googlecode.iterm2",
    "com.microsoft.VSCode",
    "com.microsoft.VSCodeInsiders",
    "dev.warp.Warp-Stable",
    "net.kovidgoyal.kitty",
    "com.github.wez.wezterm",
    "io.alacritty",
    "com.todesktop.230313mzl4w4u92"   // Cursor
]

func ensureAccessibilityTrusted() -> Bool {
    if AXIsProcessTrusted() { return true }
    // Prompt + open the Accessibility pane.
    let key = kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String
    let opts = [key: true] as CFDictionary
    _ = AXIsProcessTrustedWithOptions(opts)
    return false
}

func frontmostBundleID() -> String? {
    return NSWorkspace.shared.frontmostApplication?.bundleIdentifier
}

func tapKey(_ vk: CGKeyCode) {
    // A fresh source per call is fine for a PoC.
    let src = CGEventSource(stateID: .hidSystemState)
    if let down = CGEvent(keyboardEventSource: src, virtualKey: vk, keyDown: true) {
        down.post(tap: .cghidEventTap)
    }
    // brief gap helps TUIs register the press as discrete
    usleep(15_000) // 15ms
    if let up = CGEvent(keyboardEventSource: src, virtualKey: vk, keyDown: false) {
        up.post(tap: .cghidEventTap)
    }
    usleep(40_000) // 40ms between keys for picker reliability
}

// ---- main ----

// Guard 1: Accessibility trust (required for CGEventPost since macOS 10.14).
if !ensureAccessibilityTrusted() {
    FileHandle.standardError.write(
        "Sonari PoC: NOT a trusted Accessibility client.\n".data(using: .utf8)!)
    FileHandle.standardError.write(
        "  -> Grant Accessibility to this binary (or your terminal) in\n     System Settings > Privacy & Security > Accessibility, then rerun.\n"
        .data(using: .utf8)!)
    exit(2)
}

print("Sonari PoC: Accessibility OK.")
print("Focus the target terminal now. Injecting in 3 seconds...")
fflush(stdout)
sleep(3)

// Guard 2: Secure Event Input — synthetic keys are silently swallowed if ON.
if IsSecureEventInputEnabled() {
    FileHandle.standardError.write(
        "Sonari PoC: Secure Event Input is ENABLED — refusing to inject.\n".data(using: .utf8)!)
    FileHandle.standardError.write(
        "  -> A password field is focused, or the terminal has 'Secure Keyboard Entry' on\n     (Terminal/iTerm menu). Synthetic keystrokes would be dropped.\n     In Sonari, fall back to numeric selection here.\n"
        .data(using: .utf8)!)
    exit(3)
}

// Guard 3: confirm we're aimed at a known terminal.
let front = frontmostBundleID() ?? "<none>"
if !ALLOWED_BUNDLE_IDS.contains(front) {
    FileHandle.standardError.write(
        "Sonari PoC: frontmost app is '\(front)', not an allowed terminal — refusing to inject.\n"
        .data(using: .utf8)!)
    exit(4)
}

print("Injecting into: \(front)  ->  Down, Down, Enter")
fflush(stdout)

tapKey(VK_DOWN)
tapKey(VK_DOWN)
tapKey(VK_RETURN)

print("Done.")
