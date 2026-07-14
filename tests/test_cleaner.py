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
