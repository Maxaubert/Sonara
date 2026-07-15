"""#58: summary style/engine clamps and the style pass-through to summarize()."""
from tests.daemon_helpers import make_daemon


def test_set_config_value_clamps_summary_style():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    assert daemon.set_config_value("summary_style", "brief") is True
    assert daemon.config["summary_style"] == "brief"
    assert daemon.set_config_value("summary_style", "bogus") is False


def test_set_config_value_clamps_summary_command():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    assert daemon.set_config_value("summary_command", "codex") is True
    assert daemon.config["summary_command"] == "codex"
    assert daemon.set_config_value("summary_command", "rm -rf") is False


def test_set_summary_prompt_stores_and_resets():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    assert daemon.set_summary_prompt("natural", "CUSTOM") is True
    assert daemon.config["summary_prompts"] == {"natural": "CUSTOM"}
    assert daemon.set_summary_prompt("natural", None) is True     # reset
    assert daemon.config["summary_prompts"] == {}
    assert daemon.set_summary_prompt("bogus", "x") is False       # unknown style
    assert daemon.set_summary_prompt("brief", "   ") is False     # empty = un-firewalled


def test_set_summary_prompt_storing_the_default_resets():
    from sonara.summarizer import default_instruction
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.set_summary_prompt("tidy", "CUSTOM")
    assert daemon.set_summary_prompt("tidy", default_instruction("tidy")) is True
    assert daemon.config["summary_prompts"] == {}


def test_summary_worker_passes_style_and_custom_instruction():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.config["summary_mode"] = True
    daemon.config["summary_style"] = "brief"
    daemon.config["summary_prompts"] = {"brief": "MY RULES"}
    seen = {}
    def fake_summarize(text, **kw):
        seen.update(kw)
        return "digest"
    daemon._summarize_fn = fake_summarize
    daemon._summary_worker("fg", daemon._summary_gen.get("fg", 0), "turn text")
    assert seen["style"] == "brief"
    assert seen["instruction"] == "MY RULES"


def test_summary_worker_passes_none_instruction_when_not_customized():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.config["summary_mode"] = True
    daemon.config["summary_style"] = "tidy"
    seen = {}
    def fake_summarize(text, **kw):
        seen.update(kw)
        return "digest"
    daemon._summarize_fn = fake_summarize
    daemon._summary_worker("fg", daemon._summary_gen.get("fg", 0), "turn text")
    assert seen["style"] == "tidy"
    assert seen["instruction"] is None
