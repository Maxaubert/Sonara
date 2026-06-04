"""Tests for ProseAssembler — streamed sentence and code-fence assembly."""
from echo.assembler import ProseAssembler


def test_two_sentences_across_two_feeds_emit_both_and_hold_nothing():
    a = ProseAssembler()
    out1 = a.feed("Hello world. Second one. ", 0, False)
    assert out1 == ["Hello world.", "Second one."]
    # nothing partial held back; a final flush yields nothing
    assert a.feed("", 1, True) == []


def test_partial_is_buffered_until_completed_by_later_delta():
    a = ProseAssembler()
    assert a.feed("No terminator yet", 0, False) == []
    assert a.feed(" but now it ends.", 1, False) == ["No terminator yet but now it ends."]


def test_partial_is_flushed_on_final():
    a = ProseAssembler()
    assert a.feed("No terminator yet", 0, False) == []
    assert a.feed("", 1, True) == ["No terminator yet"]


def test_repeated_index_is_ignored():
    a = ProseAssembler()
    assert a.feed("Hello world. ", 0, False) == ["Hello world."]
    # same index again: must be ignored, no duplicate emission
    assert a.feed("Hello world. ", 0, False) == []


def test_two_sentences_in_single_delta_emit_both():
    a = ProseAssembler()
    assert a.feed("First sentence! Second sentence?", 0, True) == [
        "First sentence!",
        "Second sentence?",
    ]


def test_final_resets_state_for_reuse():
    a = ProseAssembler()
    assert a.feed("Leftover text", 0, True) == ["Leftover text"]
    # after reset, index 0 is fresh again and buffer is empty
    assert a.feed("Brand new. ", 0, False) == ["Brand new."]


def test_fence_with_info_string_emits_lang_code_block_summary():
    a = ProseAssembler()
    delta = "```python\nline one\nline two\nline three\n```"
    assert a.feed(delta, 0, True) == ["3-line python code block"]


def test_fence_without_info_string_emits_plain_code_block_summary():
    a = ProseAssembler()
    delta = "```\nalpha\nbeta\n```"
    assert a.feed(delta, 0, True) == ["2-line code block"]


def test_fence_suppresses_code_and_keeps_surrounding_prose():
    a = ProseAssembler()
    delta = "Here it is. ```python\nx = 1\ny = 2\n``` Done now."
    out = a.feed(delta, 0, True)
    assert out == ["Here it is.", "2-line python code block", "Done now."]
