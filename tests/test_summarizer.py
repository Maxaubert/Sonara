"""The out-of-band turn summarizer: a throwaway tool-disabled `claude -p` call.
All tests inject a fake runner; nothing here spawns a real process."""
import pytest

from sonara import summarizer


def _ok_runner(result="A short recap."):
    calls = []

    def run(argv, text, timeout):
        calls.append({"argv": argv, "text": text, "timeout": timeout})
        return 0, result
    return run, calls


def test_success_returns_trimmed_stdout():
    run, calls = _ok_runner("  The gist of it.\n")
    out = summarizer.summarize("long text", model="haiku", runner=run)
    assert out == "The gist of it."
    assert calls[0]["timeout"] == 20            # default timeout


def test_prompt_carries_instruction_and_delimited_message():
    # The whole prompt travels on stdin: the fixed instruction followed by the
    # message wrapped in <message> tags, so the model treats the text as content
    # to recap and never as something addressed to it.
    run, calls = _ok_runner()
    summarizer.summarize("Is this a question for you?", model="haiku", runner=run)
    sent = calls[0]["text"]
    assert sent.startswith(summarizer.INSTRUCTION)
    assert "<message>\nIs this a question for you?\n</message>" in sent


def test_instruction_has_the_not_addressed_to_you_firewall():
    # The core defense against the model answering the message instead of
    # recapping it (observed live: a question-shaped message got answered).
    assert "NEVER addressed to you" in summarizer.INSTRUCTION
    assert "<message>" in summarizer.INSTRUCTION      # names the delimiters
    assert "Input:" in summarizer.INSTRUCTION         # contains examples


def test_argv_is_headless_tool_disabled_call():
    argv = summarizer.build_argv("claude", "haiku")
    assert argv[0] == "claude"
    assert "-p" in argv
    assert "--model" in argv and argv[argv.index("--model") + 1] == "haiku"
    # --tools "" disables every tool: pure text-in/text-out
    assert "--tools" in argv and argv[argv.index("--tools") + 1] == ""
    # The prompt is NOT an argv element (it travels on stdin with the message);
    # a multi-line instruction in argv is fragile under Windows quoting.
    assert summarizer.INSTRUCTION not in argv
    # --setting-sources "" stops the child loading settings/plugins, so Sonara's
    # own hooks can NEVER fire inside the summarizer session (the recursion that
    # made the daemon summarize its own summarizer in a chime loop).
    assert ("--setting-sources" in argv
            and argv[argv.index("--setting-sources") + 1] == "")


def test_default_runner_marks_child_as_summarizer(monkeypatch):
    # Belt and braces for the recursion guard: the child env must carry
    # SONARA_SUMMARIZER=1 so the hook shim bails even if settings DO load.
    seen = {}

    class _Proc:
        returncode = 0
        stdout = b"ok"

    def fake_run(argv, **kwargs):
        seen["argv"] = argv
        seen.update(kwargs)
        return _Proc()

    monkeypatch.setattr(summarizer.subprocess, "run", fake_run)
    code, out = summarizer._default_runner(["claude", "-p"], "text", 5)
    assert code == 0 and out == "ok"
    assert seen["env"]["SONARA_SUMMARIZER"] == "1"
    assert seen["timeout"] == 5
    assert seen["input"] == b"text"


def test_nonzero_exit_returns_none():
    out = summarizer.summarize("t", model="haiku", runner=lambda a, t, s: (1, "oops"))
    assert out is None


def test_empty_stdout_returns_none():
    out = summarizer.summarize("t", model="haiku", runner=lambda a, t, s: (0, "  \n"))
    assert out is None


def test_runner_exception_returns_none():
    def boom(argv, text, timeout):
        raise RuntimeError("spawn failed")
    assert summarizer.summarize("t", model="haiku", runner=boom) is None


def test_empty_text_short_circuits_without_calling_runner():
    def never(argv, text, timeout):
        raise AssertionError("runner must not be called for empty text")
    assert summarizer.summarize("   ", model="haiku", runner=never) is None


def test_command_and_timeout_are_forwarded():
    run, calls = _ok_runner()
    summarizer.summarize("t", model="haiku", command="claude-custom",
                         timeout=5, runner=run)
    assert calls[0]["argv"][0] == "claude-custom"
    assert calls[0]["timeout"] == 5
