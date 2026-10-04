"""External engines (protocol 1.2, capability ``engines``): speech engines
the user adds at run time, such as OpenAI or a local OpenAI-compatible
server.

Each method sends one core message as is. The profile is named by the
``engine`` field (a request's ``id`` is its correlation id). A runtime that
refuses external engines (``sonarad --no-external-engines``) answers
``E_UNSUPPORTED``. A key is sent only in ``add`` (``secret``) or
``set_key``; the runtime stores it in Windows Credential Manager and never
returns it.
"""
from __future__ import annotations

from typing import Callable, Optional

Send = Callable[[str, dict], dict]


class Engines:
    """``engine_list``, ``engine_add``, ``engine_remove``, ``engine_key``,
    ``engine_test`` and ``voices`` with ``refresh``."""

    def __init__(self, send: Send):
        self._send = send

    def list(self) -> dict:
        """``engine_list``: the profiles, the built-in engines, kinds and presets."""
        return self._send("engine_list", {})

    def add(self, profile: dict, secret: Optional[str] = None, replace: bool = False) -> dict:
        """``engine_add``. It does not select the engine: ``set engine`` does."""
        fields: dict = {"engine": profile}
        if secret is not None:
            fields["secret"] = secret
        if replace:
            fields["replace"] = True
        return self._send("engine_add", fields)

    def remove(self, engine: str, forget_key: bool = True) -> dict:
        """``engine_remove``; the stored key goes too unless ``forget_key`` is false."""
        fields: dict = {"engine": engine}
        if not forget_key:
            fields["forget_key"] = False
        return self._send("engine_remove", fields)

    def set_key(self, engine: str, secret: Optional[str]) -> dict:
        """``engine_key``: store a key, or delete it with ``None``."""
        return self._send("engine_key", {"engine": engine, "secret": secret})

    def test(self, engine: str, text: Optional[str] = None, voice: Optional[str] = None,
             play: bool = True) -> dict:
        """``engine_test``: one synthesis with no fallback, at the current rate."""
        fields: dict = {"engine": engine}
        if text is not None:
            fields["text"] = text
        if voice is not None:
            fields["voice"] = voice
        if not play:
            fields["play"] = False
        return self._send("engine_test", fields)

    def voices(self, engine: str, refresh: bool = False) -> list:
        """``voices`` of one engine; ``refresh`` asks the provider again."""
        fields: dict = {"engine": engine}
        if refresh:
            fields["refresh"] = True
        return self._send("voices", fields)["voices"]
