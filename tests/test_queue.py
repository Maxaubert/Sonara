"""SpeechQueue was removed in Task 8; its tests are retired.
SpeechItem is still imported and used throughout the codebase."""
from sonara.queue import SpeechItem


def test_speech_item_is_importable():
    item = SpeechItem(id=1, session="s", kind="prose", text="hi", is_decision=False)
    assert item.text == "hi"
    assert item.is_decision is False
    assert item.mute_exempt is False
    assert item.pause_exempt is False
