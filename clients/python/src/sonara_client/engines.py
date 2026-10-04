"""External engines (protocol 1.2, capability ``engines``): speech engines
the user adds at run time, such as OpenAI or a local OpenAI-compatible
server.

Each method sends one core message as is. The profile is named by the
``engine`` field (a request's ``id`` is its correlation id). A runtime that
refuses external engines (``sonarad --no-external-engines``) answers
``E_UNSUPPORTED``. A key is sent only in ``add`` (``secret``) or
``set_key`` (or with a draft profile in ``models``, for that request only);
the runtime stores it in Windows Credential Manager and never returns it.

Sonara names no model or voice of its own (protocol 1.5, runtime 0.19.0):
``models`` and ``voices`` list the provider's live, and a profile without
one it needs says so in ``engine_list`` (``missing``).

A ``command`` engine (a program on the user's PC) is never added or changed
through the protocol: ``add`` of one, or replacing one, is ``E_FORBIDDEN``
(protocol 1.3). The user adds it locally (``sonara engines add <id> --kind
command``, or ``engines.json``); ``reload`` makes a running runtime read
``engines.json`` again. Listing, testing, selecting and removing one work.
"""
from __future__ import annotations

from typing import Callable, Optional

Send = Callable[[str, dict], dict]


class Engines:
    """``engine_list``, ``engine_add``, ``engine_remove``, ``engine_key``,
    ``engine_test``, ``engine_reload``, ``engine_models`` and ``voices`` with
    ``refresh``."""

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

    def reload(self) -> dict:
        """``engine_reload`` (protocol 1.3): the runtime reads ``engines.json``
        again. It takes no profile. Replies like ``list``, plus ``problems``."""
        return self._send("engine_reload", {})

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

    def models(self, engine: Optional[str] = None, refresh: bool = False,
               profile: Optional[dict] = None, secret: Optional[str] = None) -> dict:
        """``engine_models`` (protocol 1.5): the provider's models now, of a
        saved ``engine`` (``refresh`` asks the provider again) or of a draft
        ``profile`` (with ``secret`` for this request only). Replies
        ``{models: [{id, name}], list, takes_model, required, error?}``."""
        if profile is not None:
            fields: dict = {"profile": profile}
            if secret is not None:
                fields["secret"] = secret
            return self._send("engine_models", fields)
        if engine is None:
            raise ValueError("models needs an engine or a profile")
        fields = {"engine": engine}
        if refresh:
            fields["refresh"] = True
        return self._send("engine_models", fields)

    def voices(self, engine: str, refresh: bool = False) -> list:
        """``voices`` of one engine; ``refresh`` asks the provider again."""
        fields: dict = {"engine": engine}
        if refresh:
            fields["refresh"] = True
        return self._send("voices", fields)["voices"]
