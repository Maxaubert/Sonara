"""Chatterbox was removed (#134). What stays: a saved Chatterbox voice maps to
Kokoro's Heart on load, chatterbox_* settings are dropped, and `sonara doctor`
/ `sonara cleanup` report and remove the multi-GB leftovers without ever
touching the user's own voice clips."""
import json

from sonara import paths


def _clip(name):
    d = paths.CHATTERBOX_VOICES_DIR
    d.mkdir(parents=True, exist_ok=True)
    (d / (name + ".wav")).write_bytes(b"RIFF")


# --- voice recognition ------------------------------------------------------

def test_builtin_and_prefixed_names_are_legacy_voices():
    from sonara import chatterbox_legacy as cl
    assert cl.is_legacy_voice("cb_default")
    assert cl.is_legacy_voice("chatterbox:cherami")
    assert cl.is_legacy_voice("Chatterbox:Anything")


def test_a_clip_stem_is_a_legacy_voice_case_insensitively():
    from sonara import chatterbox_legacy as cl
    _clip("Cherami")
    assert cl.is_legacy_voice("cherami")
    assert cl.is_legacy_voice("CHERAMI")


def test_kokoro_windows_and_unknown_names_are_not_legacy_voices():
    from sonara import chatterbox_legacy as cl
    _clip("af_heart")                       # Kokoro names always won the routing
    assert not cl.is_legacy_voice("af_heart")
    assert not cl.is_legacy_voice("kokoro:af_sarah")
    assert not cl.is_legacy_voice("Microsoft David")
    assert not cl.is_legacy_voice("linus")  # no clip by that name
    assert not cl.is_legacy_voice(None)
    assert not cl.is_legacy_voice("")


def test_migrate_voice_maps_legacy_to_heart_and_keeps_the_rest():
    from sonara import chatterbox_legacy as cl
    _clip("cherami")
    assert cl.migrate_voice("cb_default") == "af_heart"
    assert cl.migrate_voice("cherami") == "af_heart"
    assert cl.migrate_voice("af_sarah") == "af_sarah"
    assert cl.migrate_voice("Microsoft Zira") == "Microsoft Zira"
    assert cl.migrate_voice(None) is None


# --- config migration -------------------------------------------------------

def _write_config(data):
    paths.ensure_sonara_dir()
    from sonara import config
    config.CONFIG_PATH.write_text(json.dumps(data), encoding="utf-8")


def test_saved_chatterbox_voice_maps_to_heart_on_load():
    from sonara.config import load_config
    _clip("cherami")
    _write_config({"voice": "cherami", "cue_voice": "cb_default"})
    cfg = load_config()
    assert cfg["voice"] == "af_heart"
    assert cfg["cue_voice"] == "af_heart"


def test_prefixed_chatterbox_voice_maps_to_heart_on_load():
    from sonara.config import load_config
    _write_config({"voice": "chatterbox:gone-clip"})
    assert load_config()["voice"] == "af_heart"


def test_non_chatterbox_voices_survive_load():
    from sonara.config import load_config
    _write_config({"voice": "af_sarah", "cue_voice": "Microsoft Zira"})
    cfg = load_config()
    assert cfg["voice"] == "af_sarah"
    assert cfg["cue_voice"] == "Microsoft Zira"


def test_chatterbox_settings_are_stripped_and_not_persisted_again():
    from sonara import config
    _write_config({"voice": "af_sarah", "chatterbox_variant": "original",
                   "chatterbox_exaggeration": 0.4,
                   "chatterbox_min_free_vram_gb": 6, "rate": 230})
    cfg = config.load_config()
    assert not [k for k in cfg if k.startswith("chatterbox_")]
    assert cfg["rate"] == 230
    config.save_config(cfg)
    saved = json.loads(config.CONFIG_PATH.read_text(encoding="utf-8"))
    assert not [k for k in saved if k.startswith("chatterbox_")]


def test_defaults_carry_no_chatterbox_keys():
    from sonara.config import DEFAULTS
    assert not [k for k in DEFAULTS if k.startswith("chatterbox")]


# --- session prefs migration ------------------------------------------------

def test_saved_session_chatterbox_voice_maps_to_heart_on_load(tmp_path):
    from sonara.session_prefs import SessionPrefs
    _clip("cherami")
    store = tmp_path / "session_prefs.json"
    store.write_text(json.dumps({
        "a": {"voice": "cherami", "name": "alpha"},
        "b": {"voice": "chatterbox:cb_default"},
        "c": {"voice": "af_nicole"},
    }), encoding="utf-8")
    prefs = SessionPrefs(store_path=store)
    assert prefs.voice("a") == "af_heart"
    assert prefs.name("a") == "alpha"
    assert prefs.voice("b") == "af_heart"
    assert prefs.voice("c") == "af_nicole"


# --- leftovers on disk (D4) -------------------------------------------------

def _make_leftovers():
    root = paths.SONARA_DIR
    venv = paths.CHATTERBOX_VENV / "Scripts"
    venv.mkdir(parents=True)
    (venv / "python.exe").write_bytes(b"x" * 1000)
    cache = paths.CHATTERBOX_MODEL_CACHE / "hf-cache"
    cache.mkdir(parents=True)
    (cache / "model.bin").write_bytes(b"x" * 2000)
    (root / "cb-client-ok.wav").write_bytes(b"x" * 10)
    (root / "chatterbox-smoke-turbo.wav").write_bytes(b"x" * 10)
    (root / "chatterbox-smoke-results.txt").write_bytes(b"x" * 5)
    _clip("cherami")
    (paths.SONARA_DIR / "voices" / "cherami-raw-backup.wav").write_bytes(b"RIFF")
    kept = root / "config.json"
    kept.write_text("{}", encoding="utf-8")


def test_leftovers_lists_venv_cache_and_smoke_files_with_sizes():
    from sonara import chatterbox_legacy as cl
    _make_leftovers()
    found = {p.name: size for p, size in cl.leftovers()}
    assert found == {
        "chatterbox-venv": 1000,
        "chatterbox": 2000,
        "cb-client-ok.wav": 10,
        "chatterbox-smoke-turbo.wav": 10,
        "chatterbox-smoke-results.txt": 5,
    }


def test_leftovers_is_empty_on_a_clean_install():
    from sonara import chatterbox_legacy as cl
    paths.ensure_sonara_dir()
    assert cl.leftovers() == []


def test_remove_leftovers_never_touches_user_voice_clips():
    from sonara import chatterbox_legacy as cl
    _make_leftovers()
    removed, failed = cl.remove_leftovers()
    assert failed == []
    assert len(removed) == 5
    assert cl.leftovers() == []
    assert (paths.CHATTERBOX_VOICES_DIR / "cherami.wav").exists()
    assert (paths.SONARA_DIR / "voices" / "cherami-raw-backup.wav").exists()
    assert (paths.SONARA_DIR / "config.json").exists()


def test_remove_leftovers_reports_a_locked_path_and_keeps_going():
    from sonara import chatterbox_legacy as cl
    _make_leftovers()

    def rmtree(path):
        if path.endswith("chatterbox-venv"):
            raise PermissionError("locked")
        import shutil
        shutil.rmtree(path)

    removed, failed = cl.remove_leftovers(rmtree=rmtree)
    assert [p.name for p, _err in failed] == ["chatterbox-venv"]
    assert len(removed) == 4


def test_format_size_is_human_readable():
    from sonara import chatterbox_legacy as cl
    assert cl.format_size(512) == "512 B"
    assert cl.format_size(3 * 1024 * 1024) == "3.0 MB"
    assert cl.format_size(int(8.7 * 1024 ** 3)) == "8.7 GB"
