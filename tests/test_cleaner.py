from sonara.cleaner import clean_markdown


def test_inline_code_backticks_removed():
    assert clean_markdown("run the `clean()` function") == "run the clean() function"


def test_bold_markers_removed_words_kept():
    assert clean_markdown("this is **very** important") == "this is very important"


def test_italic_markers_removed_words_kept():
    assert clean_markdown("this is _very_ important") == "this is very important"
    assert clean_markdown("this is *very* important") == "this is very important"


def test_double_underscore_bold_removed():
    assert clean_markdown("this is __very__ important") == "this is very important"


def test_leading_heading_hashes_removed():
    assert clean_markdown("# Title here") == "Title here"
    assert clean_markdown("## Subtitle here") == "Subtitle here"


def test_markdown_link_becomes_label():
    assert clean_markdown("see [Anthropic](https://x) for more") == "see Anthropic for more"


def test_bare_url_becomes_link_word():
    assert clean_markdown("visit https://example.com/page now") == "visit link now"


def test_table_separator_row_dropped():
    assert clean_markdown("Name\n|---|---|\nValue") == "Name Value"


def test_multiple_spaces_and_newlines_collapse_to_single_space():
    assert clean_markdown("hello    world\n\n\nthere") == "hello world there"


def test_empty_input_returns_empty():
    assert clean_markdown("") == ""


# --- normalize_for_speech (#27) -------------------------------------------

def test_normalize_for_speech_unsnakes_and_replaces_symbols():
    from sonara.cleaner import normalize_for_speech
    out = normalize_for_speech("Renamed `get_user_id` -> `fetch_user_profile` & re-ran.")
    assert "_" not in out and "`" not in out and "->" not in out and "&" not in out
    assert "get user id" in out
    assert "fetch user profile" in out
    assert " to " in out and " and " in out


def test_normalize_for_speech_keeps_plain_prose():
    from sonara.cleaner import normalize_for_speech
    text = "All tests pass. Ready to deploy."
    assert normalize_for_speech(text) == text


# --- live-prose audit (#56) --------------------------------------------------

def test_bullet_markers_stripped():
    assert clean_markdown("- item one\n- item two") == "item one item two"
    assert clean_markdown("* starred item here") == "starred item here"


def test_bare_url_keeps_sentence_period():
    # \S+ used to eat the terminator: "See link Then run it." (#56)
    assert clean_markdown("See https://example.com/docs. Then run it.") == "See link. Then run it."


def test_normalize_unsnakes_leading_underscore_identifiers():
    from sonara.cleaner import normalize_for_speech
    assert normalize_for_speech("added _voiced_upto here") == "added voiced upto here"


def test_normalize_handles_unicode_arrow_and_pipes():
    from sonara.cleaner import normalize_for_speech
    assert normalize_for_speech("daemon → assembler | channel") == "daemon to assembler channel"


def test_stabilize_ordinals_is_length_preserving():
    # The assembler applies this to RAW text pre-split; raw-offset bookkeeping
    # relies on it never changing the text's length (#56).
    from sonara.cleaner import stabilize_ordinals
    s = "1. Install\n22. Run the tests\n"
    out = stabilize_ordinals(s)
    assert len(out) == len(s)
    assert out.startswith("1: Install")
    assert "22: Run" in out
