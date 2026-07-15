from sonara.assembler import ProseAssembler, PARAGRAPH_BREAK


def test_two_sentences_across_two_feeds_emit_both_and_hold_nothing():
    a = ProseAssembler()
    out1 = a.feed("Hello world. Second one. ", 0, False)
    assert out1 == ["Hello world.", "Second one."]
    # nothing partial held back; a final flush yields nothing
    assert a.feed("", 1, True) == []


def test_partial_is_buffered_until_completed_by_later_delta():
    a = ProseAssembler()
    assert a.feed("No terminator yet", 0, False) == []
    # A terminator at the very END of the buffer may be mid-token ("3." of
    # "3.14", "daemon." of "daemon.py"), so it is held until following
    # whitespace confirms the sentence -- or the final flush delivers it (#56).
    assert a.feed(" but now it ends.", 1, False) == []
    assert a.feed(" Next thing", 2, False) == ["No terminator yet but now it ends."]


def test_partial_is_flushed_on_final():
    a = ProseAssembler()
    assert a.feed("No terminator yet", 0, False) == []
    assert a.feed("", 1, True) == ["No terminator yet"]


def test_repeated_nonzero_index_is_ignored():
    # A duplicate NON-zero index is a redelivery: ignored, no double emission.
    # (Index 0 is different: a colliding 0 means a NEW block whose predecessor
    # lost its final -- see test_lost_final_does_not_poison_next_block, #25.)
    a = ProseAssembler()
    assert a.feed("Hello world. ", 0, False) == ["Hello world."]
    assert a.feed("Second bit. ", 1, False) == ["Second bit."]
    assert a.feed("Second bit. ", 1, False) == []


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


def test_fence_spanning_multiple_feed_calls_emits_n_line_summary():
    """A code fence whose open/body/close arrive in separate feed() calls must
    produce the correct N-line summary (not prematurely closed or dropped)."""
    a = ProseAssembler()

    # Delta 0: opening fence marker + language tag
    out0 = a.feed("```python\n", 0, False)
    # The fence is open; no summary yet.
    assert out0 == []

    # Delta 1: first body line
    out1 = a.feed("x = 1\n", 1, False)
    assert out1 == []

    # Delta 2: second body line
    out2 = a.feed("y = 2\n", 2, False)
    assert out2 == []

    # Delta 3: third body line
    out3 = a.feed("z = 3\n", 3, False)
    assert out3 == []

    # Delta 4: closing fence -- summary must fire here
    out4 = a.feed("```", 4, True)
    assert out4 == ["3-line python code block"]


def test_paragraph_break_emitted_between_paragraphs():
    from sonara.assembler import ProseAssembler, PARAGRAPH_BREAK
    a = ProseAssembler()
    out = a.feed("First paragraph here.\n\nSecond paragraph here.", 0, True)
    assert "First paragraph here." in out and "Second paragraph here." in out
    assert PARAGRAPH_BREAK in out
    assert out.index("First paragraph here.") < out.index(PARAGRAPH_BREAK) < out.index("Second paragraph here.")


def test_no_paragraph_break_within_one_paragraph():
    from sonara.assembler import ProseAssembler, PARAGRAPH_BREAK
    a = ProseAssembler()
    out = a.feed("One sentence. Two sentences. Still one paragraph.", 0, True)
    assert PARAGRAPH_BREAK not in out


def test_three_paragraphs_two_breaks():
    from sonara.assembler import ProseAssembler, PARAGRAPH_BREAK
    a = ProseAssembler()
    out = a.feed("Para one.\n\nPara two.\n\nPara three.", 0, True)
    assert out.count(PARAGRAPH_BREAK) == 2


def test_blank_line_split_across_deltas_still_breaks_paragraphs():
    """A blank line (\\n\\n) straddling two streamed deltas must still produce a
    PARAGRAPH_BREAK and must not merge the two paragraphs. Regression: the buffer
    was overwritten with whitespace-collapsed text between deltas, erasing the
    trailing newline of a straddling blank line, so the heading merged into the
    next paragraph and reading stalled/garbled at every blank line."""
    from sonara.assembler import ProseAssembler, PARAGRAPH_BREAK
    a = ProseAssembler()
    out = []
    out += a.feed("The headline issue\n", 0, False)        # heading, no period, ends with \n
    out += a.feed("\nThe next part is here.\n", 1, False)   # starts with \n -> blank line straddles
    out += a.feed("", 2, True)
    texts = [c for c in out if c is not PARAGRAPH_BREAK]
    assert "The headline issue" in texts, f"heading not emitted as its own chunk: {texts}"
    assert any("next part is here" in t for t in texts), texts
    assert PARAGRAPH_BREAK in out, "paragraph break lost across deltas"
    assert out.index("The headline issue") < out.index(PARAGRAPH_BREAK)
    # no word-joining across the lost newline/space
    assert not any("issueThe" in t or "issue The next" in t for t in texts), texts


# --- deep audit #25: order inversion, lost-final poisoning, fence closing ----

def test_lead_in_before_fence_spoken_before_fence_summary():
    # An unterminated lead-in ("Here is the code:") used to be spoken AFTER the
    # fence's "N-line code block" summary (held as remainder until final flush),
    # inverting the spoken order for an eyes-free user (deep audit #25).
    a = ProseAssembler()
    out = a.feed("Here is the code:\n```python\nx = 1\n```\n", 0, True)
    texts = [t for t in out if t is not PARAGRAPH_BREAK]
    lead = next(i for i, t in enumerate(texts) if "Here is the code" in t)
    fence = next(i for i, t in enumerate(texts) if "code block" in t)
    assert lead < fence


def test_lost_final_does_not_poison_next_block():
    # A lost final delta left _seen populated; the next block restarts at index
    # 0, collided, and its opening deltas were silently DROPPED (deep audit #25).
    a = ProseAssembler()
    a.feed("First block sentence. ", 0, False)          # final never arrives
    out = a.feed("Second block starts here. ", 0, False)  # new block, index 0
    texts = [t for t in out if t is not PARAGRAPH_BREAK]
    assert any("Second block starts here." in t for t in texts)


def test_fence_with_inner_backtick_literal_does_not_close_early():
    # Closing-fence detection matched ANY ``` occurrence, so code CONTAINING a
    # ``` literal (or 4-backtick fences) closed early and leaked code as prose
    # (deep audit #25). A closing fence is its own all-backticks line.
    a = ProseAssembler()
    out = a.feed("```md\nuse ``` to open\nmore code\n```\nAfter text. ", 0, True)
    texts = [t for t in out if t is not PARAGRAPH_BREAK]
    summaries = [t for t in texts if "code block" in t]
    assert len(summaries) == 1
    assert summaries[0].startswith("2-line")             # both lines counted
    assert any("After text." in t for t in texts)        # prose resumes cleanly
    assert not any("more code" in t for t in texts)      # code never leaks


# --- live-prose audit #56: offset desync, mid-token splits, tail blobs -------

def test_markdown_pair_straddling_deltas_does_not_chop_chars():
    # "**Bottom line: ... Ship it now.**" split across deltas used to speak
    # literal asterisks then "hip it now." -- the cleaned-offset bookkeeping
    # desynced when the closing marker arrived late (#56).
    a = ProseAssembler()
    out = a.feed("**Bottom line: it works. ", 0, False)
    out += a.feed("Ship it now.**", 1, True)
    assert out == ["Bottom line: it works.", "Ship it now."]


def test_inline_code_straddling_deltas_keeps_all_chars():
    # `daemon.py` split across deltas used to drop the "p" and speak a raw
    # backtick (#56).
    a = ProseAssembler()
    out = a.feed("I updated `daemon.", 0, False)
    out += a.feed("py` and restarted the service.", 1, True)
    assert out == ["I updated daemon.py and restarted the service."]


def test_decimal_straddling_deltas_stays_one_sentence():
    # A delta boundary right after "3." used to emit "The cost is 3." as its
    # own premature utterance (#56).
    a = ProseAssembler()
    out = a.feed("The cost is 3.", 0, False)
    out += a.feed("5 million dollars total.", 1, True)
    assert out == ["The cost is 3.5 million dollars total."]


def test_closing_bullet_list_emits_one_chunk_per_item():
    # The classic end-of-turn bullet list used to leave the final flush as ONE
    # unpunctuated 300+ char blob that Chatterbox hard-split mid-clause (#56).
    a = ProseAssembler()
    text = ("Here is what changed.\n"
            "- Fixed the race in the daemon\n"
            "- Added a regression test\n"
            "- Deployed the new build")
    out = a.feed(text, 0, True)
    assert out == ["Here is what changed.",
                   "Fixed the race in the daemon",
                   "Added a regression test",
                   "Deployed the new build"]


def test_snake_case_is_unsnaked_on_the_live_path():
    # Live prose used to get only clean_markdown, whose emphasis rule GLUED
    # snake_case into non-words ("handlemessage") before the voice (#56).
    a = ProseAssembler()
    out = a.feed("Renamed handle_message and added _voiced_upto. ", 0, True)
    assert out == ["Renamed handle message and added voiced upto."]


def test_arrow_and_ampersand_normalized_on_the_live_path():
    a = ProseAssembler()
    out = a.feed("The flow is daemon -> assembler & channel. ", 0, True)
    assert out == ["The flow is daemon to assembler and channel."]


def test_punctuation_only_chunk_is_not_emitted():
    a = ProseAssembler()
    assert a.feed("...", 0, True) == []


def test_numbered_list_ordinals_attach_to_their_items():
    # "1. Install the package\n2. Run the tests" was chunked as "1." /
    # "Install the package 2." / "Run the tests" -- the ordinal attached to the
    # WRONG spoken item (deep audit #25).
    a = ProseAssembler()
    out = a.feed("1. Install the package\n2. Run the tests\n", 0, True)
    texts = [t for t in out if t is not PARAGRAPH_BREAK]
    assert not any(t.rstrip().endswith("2.") for t in texts)   # no orphan ordinal
    two = next(t for t in texts if "Run the tests" in t)
    assert "2" in two                                     # ordinal rides its item
