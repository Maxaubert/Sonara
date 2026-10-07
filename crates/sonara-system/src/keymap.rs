//! The keymap: hotkey actions, their default chords, `keymap.json` in the
//! home, and the Windows key tables (ported from the Python `keymap.py` and
//! `platform/windows/keytables.py`).
//!
//! `keymap.json` holds only the user's overrides (`{"mute": {"key": "k",
//! "mods": ["ctrl", "alt"]}}`); a binding with no key is an explicit
//! unbind. Loading never fails: a missing or corrupt file gives the
//! defaults, unknown actions are ignored, and an entry naming a key or
//! modifier this table lacks is reported and skipped instead of taking
//! every hotkey down (#38). `bind` validates before anything is written.
use crate::platform::KeyboardLayout;
use crate::state_file;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// What a hotkey does. The host maps each action to protocol controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Action {
    /// Read the current message again from the top (`control restart`).
    Restart,
    /// Flush: stop only the session being read (`control flush`, #228;
    /// `control stop` without channels).
    Flush,
    /// Pause or resume (`control toggle`).
    Pause,
    /// Mute cycle (agent mute levels 0, 1, 2; else core mute and unmute).
    Mute,
    /// Switch to the next channel (`control next_channel`).
    NextChannel,
    /// Speak faster (rate + 25).
    Faster,
    /// Speak slower (rate - 25).
    Slower,
    /// The previous question of the message's question set; from the
    /// message text, question 1; on question 1, question 1 again
    /// (`control previous_question`, #283).
    PreviousQuestion,
    /// The next question of the message's question set; from the message
    /// text, or a set not started yet, question 1 (`control
    /// next_question`, #283).
    NextQuestion,
}

impl Action {
    /// Every action. Its order is the registration id (`id`): a new
    /// action is appended, so the ids of the others never move.
    pub const ALL: [Action; 9] = [
        Action::Restart,
        Action::Flush,
        Action::Pause,
        Action::Mute,
        Action::NextChannel,
        Action::Faster,
        Action::Slower,
        Action::PreviousQuestion,
        Action::NextQuestion,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            Action::Restart => "restart",
            Action::Flush => "flush",
            Action::Pause => "pause",
            Action::Mute => "mute",
            Action::NextChannel => "next_channel",
            Action::Faster => "faster",
            Action::Slower => "slower",
            Action::PreviousQuestion => "previous_question",
            Action::NextQuestion => "next_question",
        }
    }

    /// The action of a name. The Python plugin's names (`nav_start`,
    /// `next_session`) are accepted too, so its keymap carries over.
    pub fn parse(name: &str) -> Option<Action> {
        Some(match name {
            "restart" | "nav_start" => Action::Restart,
            "flush" => Action::Flush,
            "pause" => Action::Pause,
            "mute" => Action::Mute,
            "next_channel" | "next_session" => Action::NextChannel,
            "faster" => Action::Faster,
            "slower" => Action::Slower,
            "previous_question" => Action::PreviousQuestion,
            "next_question" => Action::NextQuestion,
            _ => return None,
        })
    }

    /// A rapid repeat of a toggle is ignored (pause and mute).
    pub fn debounced(&self) -> bool {
        matches!(self, Action::Pause | Action::Mute)
    }

    /// The id the hotkey is registered under (1-based, stable).
    pub fn id(&self) -> i32 {
        Action::ALL.iter().position(|a| a == self).unwrap_or(0) as i32 + 1
    }

    pub fn from_id(id: i32) -> Option<Action> {
        usize::try_from(id - 1)
            .ok()
            .and_then(|i| Action::ALL.get(i).copied())
    }
}

/// RegisterHotKey's MOD_NOREPEAT, added at registration (holding a key does
/// not repeat the action).
pub const MOD_NOREPEAT: u32 = 0x4000;
pub const MOD_ALT: u32 = 0x0001;
pub const MOD_CTRL: u32 = 0x0002;
pub const MOD_SHIFT: u32 = 0x0004;
pub const MOD_WIN: u32 = 0x0008;

/// The default chord (#160, user decision 2026-10-02): Ctrl+Alt. Windows
/// owns Win+Alt+Up/Down/M/P. Ctrl+Alt is AltGr on many European layouts,
/// so a clash is a warning whose fix is a rebind with Win.
pub const DEFAULT_MODS: [&str; 2] = ["ctrl", "alt"];

/// Action -> default key; pause, faster and slower ship unbound. Left
/// and Right move between a message's questions (#283); Intel graphics
/// drivers may own Ctrl+Alt+arrows (screen rotation): a clash is a doctor
/// and settings warning whose fix is turning that off or a rebind.
pub const DEFAULT_KEYS: [(Action, &str); 6] = [
    (Action::Restart, "up"),
    (Action::Flush, "down"),
    (Action::Mute, "m"),
    (Action::NextChannel, "p"),
    (Action::PreviousQuestion, "left"),
    (Action::NextQuestion, "right"),
];

/// Named keys and their virtual-key codes (letters and digits are added by
/// `key_code`: their codes are their ASCII values).
const NAMED_KEYS: &[(&str, u32)] = &[
    ("period", 0xBE),
    (".", 0xBE),
    ("rightbracket", 0xDD),
    ("]", 0xDD),
    ("leftbracket", 0xDB),
    ("[", 0xDB),
    ("left", 0x25),
    ("leftarrow", 0x25),
    ("up", 0x26),
    ("uparrow", 0x26),
    ("right", 0x27),
    ("rightarrow", 0x27),
    ("down", 0x28),
    ("downarrow", 0x28),
    ("home", 0x24),
    ("end", 0x23),
    ("pageup", 0x21),
    ("pagedown", 0x22),
];

const MODS: &[(&str, u32)] = &[
    ("alt", MOD_ALT),
    ("ctrl", MOD_CTRL),
    ("control", MOD_CTRL),
    ("shift", MOD_SHIFT),
    ("win", MOD_WIN),
    ("cmd", MOD_WIN),
];

/// The virtual-key code of a key name (case-insensitive).
pub fn key_code(name: &str) -> Option<u32> {
    let n = name.to_ascii_lowercase();
    if let Some((_, vk)) = NAMED_KEYS.iter().find(|(k, _)| *k == n) {
        return Some(*vk);
    }
    let mut chars = n.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_lowercase() => Some(c.to_ascii_uppercase() as u32),
        (Some(c), None) if c.is_ascii_digit() => Some(c as u32),
        _ => None,
    }
}

pub fn mod_mask(name: &str) -> Option<u32> {
    let n = name.to_ascii_lowercase();
    MODS.iter().find(|(m, _)| *m == n).map(|(_, v)| *v)
}

/// Every bindable key name, sorted (the settings page validates a capture
/// against it).
pub fn key_names() -> Vec<String> {
    let mut v: Vec<String> = NAMED_KEYS.iter().map(|(k, _)| k.to_string()).collect();
    v.extend(('a'..='z').map(|c| c.to_string()));
    v.extend(('0'..='9').map(|c| c.to_string()));
    v.sort();
    v
}

pub fn mod_names() -> Vec<String> {
    let mut v: Vec<String> = MODS.iter().map(|(m, _)| m.to_string()).collect();
    v.sort();
    v
}

/// One action's chord: `key` `None` (or empty) is unbound.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Binding {
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub mods: Vec<String>,
}

impl Binding {
    pub fn new(key: &str, mods: &[&str]) -> Self {
        Binding {
            key: Some(key.to_string()),
            mods: mods.iter().map(|m| m.to_string()).collect(),
        }
    }

    pub fn unbound() -> Self {
        Binding::default()
    }

    pub fn is_bound(&self) -> bool {
        self.key.as_deref().is_some_and(|k| !k.is_empty())
    }
}

/// Every action with its binding, in `Action::ALL` order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keymap {
    pub bindings: Vec<(Action, Binding)>,
}

impl Keymap {
    pub fn get(&self, action: Action) -> &Binding {
        &self
            .bindings
            .iter()
            .find(|(a, _)| *a == action)
            .expect("every action has a binding")
            .1
    }

    fn set(&mut self, action: Action, b: Binding) {
        if let Some(slot) = self.bindings.iter_mut().find(|(a, _)| *a == action) {
            slot.1 = b;
        }
    }
}

fn default_binding(action: Action) -> Binding {
    DEFAULT_KEYS
        .iter()
        .find(|(a, _)| *a == action)
        .map(|(_, k)| Binding::new(k, &DEFAULT_MODS))
        .unwrap_or_default()
}

/// The default keymap: Ctrl+Alt+Up/Down/M/P/Left/Right, the rest unbound.
pub fn defaults() -> Keymap {
    Keymap {
        bindings: Action::ALL
            .iter()
            .map(|a| (*a, default_binding(*a)))
            .collect(),
    }
}

type Overrides = BTreeMap<String, Binding>;

/// The user's overrides, keyed by the current action names; unknown actions
/// and entries that are not `{key, mods}` are dropped.
fn read_overrides(path: &Path) -> Vec<(Action, Binding)> {
    let Some(serde_json::Value::Object(raw)) = state_file::read::<serde_json::Value>(path) else {
        return Vec::new();
    };
    let mut out: Vec<(Action, Binding)> = Vec::new();
    for (name, v) in raw {
        let Some(action) = Action::parse(&name) else {
            continue;
        };
        let Ok(b) = serde_json::from_value::<Binding>(v) else {
            continue;
        };
        // The current name wins over a legacy alias for the same action.
        let current = action.as_str() == name;
        match out.iter_mut().find(|(a, _)| *a == action) {
            Some(slot) if current => slot.1 = b,
            Some(_) => {}
            None => out.push((action, b)),
        }
    }
    out
}

fn write_overrides(path: &Path, list: &[(Action, Binding)]) -> Result<(), String> {
    let map: Overrides = list
        .iter()
        .map(|(a, b)| (a.as_str().to_string(), b.clone()))
        .collect();
    state_file::write(path, &map).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// The defaults with the user's overrides from `path` on top.
pub fn load(path: &Path) -> Keymap {
    let mut km = defaults();
    for (a, b) in read_overrides(path) {
        km.set(a, b);
    }
    km
}

/// A binding ready to register.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resolved {
    pub action: Action,
    pub vk: u32,
    /// Modifier mask without MOD_NOREPEAT.
    pub mods: u32,
}

/// The bound actions as key codes and masks. An entry with an unknown key
/// or modifier is skipped and reported, never fatal for the others.
pub fn resolve(km: &Keymap) -> (Vec<Resolved>, Vec<String>) {
    let mut out = Vec::new();
    let mut problems = Vec::new();
    for (action, b) in &km.bindings {
        if !b.is_bound() {
            continue;
        }
        let key = b.key.as_deref().unwrap_or_default();
        let Some(vk) = key_code(key) else {
            problems.push(format!("{}: unknown key '{key}'", action.as_str()));
            continue;
        };
        let mut mask = 0;
        let mut ok = true;
        for m in &b.mods {
            match mod_mask(m) {
                Some(v) => mask |= v,
                None => {
                    problems.push(format!("{}: unknown modifier '{m}'", action.as_str()));
                    ok = false;
                }
            }
        }
        if ok {
            out.push(Resolved {
                action: *action,
                vk,
                mods: mask,
            });
        }
    }
    (out, problems)
}

/// Bind `action` to `key` + `mods` in `path`. Validated first, so a bad
/// binding never reaches the file (#38): the key and modifiers must exist
/// and at least one of Ctrl, Alt or Win is held, since a global hotkey
/// without one takes that key away from every app (E13).
pub fn bind(path: &Path, action: Action, key: &str, mods: &[String]) -> Result<(), String> {
    let key = key.trim().to_ascii_lowercase();
    if key.is_empty() {
        return Err("empty key".into());
    }
    if key_code(&key).is_none() {
        return Err(format!("unsupported key '{key}'"));
    }
    let mut held = 0;
    let mut clean: Vec<String> = Vec::new();
    for m in mods {
        let mask = mod_mask(m).ok_or_else(|| format!("unsupported modifier '{m}'"))?;
        held |= mask;
        let m = m.to_ascii_lowercase();
        if !clean.contains(&m) {
            clean.push(m);
        }
    }
    if held & (MOD_CTRL | MOD_ALT | MOD_WIN) == 0 {
        return Err(
            "a hotkey needs Ctrl, Alt or Win: without one it would take that key away \
                    from every app"
                .into(),
        );
    }
    let mut list = read_overrides(path);
    let b = Binding {
        key: Some(key),
        mods: clean,
    };
    match list.iter_mut().find(|(a, _)| *a == action) {
        Some(slot) => slot.1 = b,
        None => list.push((action, b)),
    }
    write_overrides(path, &list)
}

/// No hotkey for `action`: an explicit unbound override when it has a
/// default binding, else its user binding is dropped.
pub fn unbind(path: &Path, action: Action) -> Result<(), String> {
    let mut list = read_overrides(path);
    list.retain(|(a, _)| *a != action);
    if default_binding(action).is_bound() {
        list.push((action, Binding::unbound()));
    }
    write_overrides(path, &list)
}

/// Replace the user's overrides with the default bindings (#160); every
/// override, explicit unbinds included, is dropped.
pub fn reset(path: &Path) -> Result<(), String> {
    let list: Vec<(Action, Binding)> = defaults()
        .bindings
        .into_iter()
        .filter(|(_, b)| b.is_bound())
        .collect();
    write_overrides(path, &list)
}

/// How a chord reads: `Ctrl+Alt+Up` (Win first, as Windows writes it).
pub fn combo_label(mods: u32, vk: u32) -> String {
    let mut parts: Vec<String> = [
        (MOD_WIN, "Win"),
        (MOD_CTRL, "Ctrl"),
        (MOD_SHIFT, "Shift"),
        (MOD_ALT, "Alt"),
    ]
    .iter()
    .filter(|(m, _)| mods & m != 0)
    .map(|(_, n)| n.to_string())
    .collect();
    let named = [
        (0xBE, "."),
        (0xDD, "]"),
        (0xDB, "["),
        (0x25, "Left"),
        (0x26, "Up"),
        (0x27, "Right"),
        (0x28, "Down"),
        (0x24, "Home"),
        (0x23, "End"),
        (0x21, "PageUp"),
        (0x22, "PageDown"),
    ];
    let label = match named.iter().find(|(v, _)| *v == vk) {
        Some((_, n)) => n.to_string(),
        None if (0x41..=0x5A).contains(&vk) || (0x30..=0x39).contains(&vk) => {
            char::from_u32(vk).map(String::from).unwrap_or_default()
        }
        None => format!("key{vk}"),
    };
    parts.push(label);
    parts.join("+")
}

/// A Ctrl+Alt hotkey that is AltGr typing a character on this layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AltGr {
    pub action: Action,
    pub combo: String,
    pub character: String,
}

/// Each Ctrl+Alt hotkey (without Win) that is AltGr typing a character on
/// the current layout (German AltGr+M types the micro sign): that hotkey
/// takes the character away from every app (E16).
pub fn altgr_conflicts(resolved: &[Resolved], layout: &dyn KeyboardLayout) -> Vec<AltGr> {
    let ctrl_alt = MOD_CTRL | MOD_ALT;
    resolved
        .iter()
        .filter(|r| r.mods & ctrl_alt == ctrl_alt && r.mods & MOD_WIN == 0)
        .filter_map(|r| {
            layout
                .altgr_char(r.vk, r.mods & MOD_SHIFT != 0)
                .map(|ch| AltGr {
                    action: r.action,
                    combo: combo_label(r.mods, r.vk),
                    character: ch,
                })
        })
        .collect()
}
