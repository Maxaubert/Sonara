import re

from sonara import daemon


def test_select_token_reuses_wellformed_prior():
    tok = "ab" * 32
    assert daemon._select_token({"token": tok}) == tok


def test_select_token_mints_fresh_when_missing_or_malformed():
    for prior in ({}, {"token": None}, {"token": "short"}, {"token": "Z" * 64}, None):
        t = daemon._select_token(prior)
        assert re.fullmatch(r"[0-9a-f]{64}", t)


def test_select_token_fresh_tokens_differ():
    assert daemon._select_token({}) != daemon._select_token({})
