//! Keymap rules, ported from the Python `tests/test_keymap.py` and
//! `tests/test_keymap_validation.py`.
mod common;

use common::{read_json, tmp};
use serde_json::json;
use sonara_system::fake::{Fake, FakeAltGr};
use sonara_system::keymap::{self, Action, Binding};

fn key_of(km: &keymap::Keymap, a: Action) -> Option<String> {
    km.get(a).key.clone()
}

fn mods(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn defaults_are_ctrl_alt_up_down_m_p() {
    // #160: Windows owns Win+Alt+Up/Down/M/P, so the defaults stay Ctrl+Alt.
    let km = keymap::defaults();
    assert_eq!(key_of(&km, Action::Restart).as_deref(), Some("up"));
    assert_eq!(key_of(&km, Action::Flush).as_deref(), Some("down"));
    assert_eq!(key_of(&km, Action::Mute).as_deref(), Some("m"));
    assert_eq!(key_of(&km, Action::NextChannel).as_deref(), Some("p"));
    for a in [Action::Pause, Action::Faster, Action::Slower] {
        assert!(!km.get(a).is_bound(), "{a:?} ships unbound");
    }
    for (_, b) in km.bindings.iter().filter(|(_, b)| b.is_bound()) {
        assert_eq!(b.mods, mods(&["ctrl", "alt"]));
    }
}

#[test]
fn no_two_defaults_share_a_key() {
    let km = keymap::defaults();
    let keys: Vec<_> = km
        .bindings
        .iter()
        .filter_map(|(_, b)| b.key.clone())
        .collect();
    let mut dedup = keys.clone();
    dedup.sort();
    dedup.dedup();
    assert_eq!(keys.len(), dedup.len());
}

#[test]
fn default_keys_bind_ctrl_alt_left_and_right() {
    // #283: Ctrl+Alt+Left/Right move between a message's questions.
    let km = keymap::defaults();
    assert_eq!(
        key_of(&km, Action::PreviousQuestion).as_deref(),
        Some("left")
    );
    assert_eq!(key_of(&km, Action::NextQuestion).as_deref(), Some("right"));
    let (r, _) = keymap::resolve(&km);
    let right = r.iter().find(|r| r.action == Action::NextQuestion).unwrap();
    assert_eq!(keymap::combo_label(right.mods, right.vk), "Ctrl+Alt+Right");
    assert!(!Action::NextQuestion.debounced() && !Action::PreviousQuestion.debounced());
}

#[test]
fn question_actions_have_stable_ids_after_the_old_ones() {
    // The registration ids of the older actions never move (#283).
    let old = [
        Action::Restart,
        Action::Flush,
        Action::Pause,
        Action::Mute,
        Action::NextChannel,
        Action::Faster,
        Action::Slower,
    ];
    for (i, a) in old.iter().enumerate() {
        assert_eq!(a.id(), i as i32 + 1, "{a:?}");
    }
    assert_eq!(Action::PreviousQuestion.id(), 8);
    assert_eq!(Action::NextQuestion.id(), 9);
    assert_eq!(
        Action::parse("previous_question"),
        Some(Action::PreviousQuestion)
    );
    assert_eq!(Action::parse("next_question"), Some(Action::NextQuestion));
}

#[test]
fn resolve_gives_windows_codes_and_masks() {
    let (r, problems) = keymap::resolve(&keymap::defaults());
    assert!(problems.is_empty());
    let up = r.iter().find(|r| r.action == Action::Restart).unwrap();
    assert_eq!(up.vk, 0x26);
    assert_eq!(up.mods, keymap::MOD_CTRL | keymap::MOD_ALT);
    assert_eq!(keymap::key_code("p"), Some(0x50));
    assert_eq!(keymap::key_code("."), Some(0xBE));
    assert_eq!(keymap::key_code("7"), Some(0x37));
    assert_eq!(keymap::key_code("escape"), None);
}

#[test]
fn every_letter_and_digit_is_bindable() {
    for c in ('a'..='z').chain('0'..='9') {
        assert!(keymap::key_code(&c.to_string()).is_some(), "{c}");
        assert!(keymap::key_names().contains(&c.to_string()));
    }
}

#[test]
fn load_without_a_file_is_the_defaults() {
    let path = tmp("km-none").join("keymap.json");
    assert_eq!(keymap::load(&path), keymap::defaults());
}

#[test]
fn load_tolerates_a_corrupt_file() {
    let path = tmp("km-bad").join("keymap.json");
    std::fs::write(&path, "{ not json").unwrap();
    assert_eq!(keymap::load(&path), keymap::defaults());
}

#[test]
fn load_merges_overrides_and_drops_unknown_actions() {
    let path = tmp("km-merge").join("keymap.json");
    std::fs::write(
        &path,
        json!({"pause": {"key": "x", "mods": ["win"]},
               "nav_prev": {"key": "left", "mods": ["ctrl", "alt"]},
               "stop": {"key": "s", "mods": ["ctrl"]}})
        .to_string(),
    )
    .unwrap();
    let km = keymap::load(&path);
    assert_eq!(km.get(Action::Pause), &Binding::new("x", &["win"]));
    assert_eq!(
        km.get(Action::Restart),
        keymap::defaults().get(Action::Restart)
    );
}

#[test]
fn load_accepts_the_python_action_names() {
    // The Python plugin's keymap.json carries over (nav_start, next_session).
    let path = tmp("km-legacy").join("keymap.json");
    std::fs::write(
        &path,
        json!({"nav_start": {"key": "home", "mods": ["win", "alt"]},
               "next_session": {"key": "n", "mods": ["ctrl", "alt"]}})
        .to_string(),
    )
    .unwrap();
    let km = keymap::load(&path);
    assert_eq!(
        km.get(Action::Restart),
        &Binding::new("home", &["win", "alt"])
    );
    assert_eq!(
        km.get(Action::NextChannel),
        &Binding::new("n", &["ctrl", "alt"])
    );
}

#[test]
fn a_bad_entry_is_skipped_not_fatal_for_the_others() {
    // #38: one bad entry used to disable every hotkey.
    let path = tmp("km-skip").join("keymap.json");
    std::fs::write(
        &path,
        json!({"mute": {"key": "zzz", "mods": ["ctrl"]},
               "pause": {"key": "p", "mods": ["hyper"]}})
        .to_string(),
    )
    .unwrap();
    let (r, problems) = keymap::resolve(&keymap::load(&path));
    assert_eq!(problems.len(), 2, "{problems:?}");
    let actions: Vec<_> = r.iter().map(|r| r.action).collect();
    assert!(actions.contains(&Action::Restart));
    assert!(!actions.contains(&Action::Mute));
}

#[test]
fn bind_persists_an_override() {
    let path = tmp("km-bind").join("keymap.json");
    keymap::bind(&path, Action::Mute, "K", &mods(&["Ctrl", "shift"])).unwrap();
    assert_eq!(
        keymap::load(&path).get(Action::Mute),
        &Binding::new("k", &["ctrl", "shift"])
    );
}

#[test]
fn bind_refuses_unknown_keys_and_modifiers_before_writing() {
    let path = tmp("km-refuse").join("keymap.json");
    let e = keymap::bind(&path, Action::Mute, "escape", &mods(&["ctrl", "alt"])).unwrap_err();
    assert!(e.contains("key"), "{e}");
    let e = keymap::bind(&path, Action::Mute, "m", &mods(&["hyper"])).unwrap_err();
    assert!(e.contains("modifier"), "{e}");
    assert!(!path.exists(), "nothing persisted");
}

#[test]
fn bind_refuses_a_hotkey_without_ctrl_alt_or_win() {
    // E13: a bare 'm' (or Shift+M) would stop typing m in every app.
    let path = tmp("km-e13").join("keymap.json");
    for m in [vec![], mods(&["shift"])] {
        let e = keymap::bind(&path, Action::Mute, "m", &m).unwrap_err();
        assert!(e.contains("Ctrl"), "{e}");
    }
    assert!(!path.exists());
    keymap::bind(&path, Action::Mute, "m", &mods(&["win", "shift"])).unwrap();
}

#[test]
fn unbind_writes_an_explicit_override_for_a_default_binding() {
    let path = tmp("km-unbind").join("keymap.json");
    keymap::unbind(&path, Action::Restart).unwrap();
    assert_eq!(read_json(&path)["restart"]["key"], json!(null));
    let (r, _) = keymap::resolve(&keymap::load(&path));
    assert!(!r.iter().any(|r| r.action == Action::Restart));
}

#[test]
fn unbind_of_an_action_without_a_default_drops_its_binding() {
    let path = tmp("km-unbind2").join("keymap.json");
    keymap::bind(&path, Action::Faster, "]", &mods(&["alt"])).unwrap();
    keymap::unbind(&path, Action::Faster).unwrap();
    assert!(read_json(&path).get("faster").is_none());
}

#[test]
fn a_rewrite_prunes_unknown_and_legacy_names() {
    let path = tmp("km-prune").join("keymap.json");
    std::fs::write(
        &path,
        json!({"nav_prev": {"key": "left", "mods": ["ctrl", "alt"]},
               "next_session": {"key": "j", "mods": ["ctrl", "alt"]}})
        .to_string(),
    )
    .unwrap();
    keymap::unbind(&path, Action::Mute).unwrap();
    let on_disk = read_json(&path);
    let keys: Vec<_> = on_disk.as_object().unwrap().keys().cloned().collect();
    assert_eq!(keys, vec!["mute".to_string(), "next_channel".to_string()]);
}

#[test]
fn reset_restores_the_defaults() {
    let path = tmp("km-reset").join("keymap.json");
    keymap::bind(&path, Action::Faster, "f", &mods(&["ctrl", "alt"])).unwrap();
    keymap::unbind(&path, Action::Restart).unwrap();
    keymap::reset(&path).unwrap();
    assert_eq!(keymap::load(&path), keymap::defaults());
}

#[test]
fn combo_labels_read_like_windows() {
    let ca = keymap::MOD_CTRL | keymap::MOD_ALT;
    assert_eq!(keymap::combo_label(ca, 0x26), "Ctrl+Alt+Up");
    assert_eq!(
        keymap::combo_label(keymap::MOD_WIN | keymap::MOD_ALT, 0x24),
        "Win+Alt+Home"
    );
    assert_eq!(
        keymap::combo_label(ca | keymap::MOD_SHIFT, 0x4D),
        "Ctrl+Shift+Alt+M"
    );
    assert_eq!(keymap::combo_label(ca, 0x37), "Ctrl+Alt+7");
}

#[test]
fn altgr_conflicts_flag_ctrl_alt_hotkeys_that_type_a_character() {
    // E16: German AltGr+M types the micro sign, so Ctrl+Alt+M eats it. A Win
    // chord never matches AltGr.
    let fake = Fake::new();
    fake.edit(|w| {
        w.altgr.push(FakeAltGr {
            vk: 0x4D,
            shift: false,
            char: "\u{b5}".into(),
        })
    });
    let layout = fake.platform().layout;
    let (r, _) = keymap::resolve(&keymap::defaults());
    let found = keymap::altgr_conflicts(&r, layout.as_ref());
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].action, Action::Mute);
    assert_eq!(found[0].combo, "Ctrl+Alt+M");
    assert_eq!(found[0].character, "\u{b5}");
    let path = tmp("km-altgr").join("keymap.json");
    keymap::bind(&path, Action::Mute, "m", &mods(&["ctrl", "alt", "win"])).unwrap();
    let (r, _) = keymap::resolve(&keymap::load(&path));
    assert!(keymap::altgr_conflicts(&r, layout.as_ref()).is_empty());
}

#[test]
fn action_names_and_ids_round_trip() {
    for a in Action::ALL {
        assert_eq!(Action::parse(a.as_str()), Some(a));
        assert_eq!(Action::from_id(a.id()), Some(a));
    }
    assert_eq!(Action::from_id(0), None);
    assert_eq!(Action::parse("bogus"), None);
}
