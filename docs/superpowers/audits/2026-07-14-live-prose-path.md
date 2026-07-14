# Audit: live (summary-off) prose speech path

**Date:** 2026-07-14
**Trigger:** user report — with summary mode off, live-spoken prose garbled near the end of the last paragraph of a turn; summary mode sounds fine.
**Method:** 4-dimension workflow audit (assembler final-flush, cleaner coverage, chatterbox chunking, speak-loop batching), 23 agents, every finding adversarially verified with executable repros against the real modules. 17 confirmed, 2 refuted.

Pipeline under audit: PROSE deltas -> `assembler.feed` (`clean_markdown` + sentence split) -> per-sentence SpeechItems -> speak loop (one item per `speaker.speak`) -> `chatterbox.split_text` -> worker `_split_text` (280-char cap) -> `model.generate` per chunk.

## Confirmed findings

### Critical

1. **Turn-final flush emits the whole unpunctuated tail as ONE blob** — `assembler.py:243-253`. A closing dash-bullet list gets no terminal punctuation (`_LIST_ITEM_END` at `cleaner.py:31` only terminates numbered items), `_WHITESPACE` collapses the newlines, the sentence splitter finds no `[.!?]`, so the whole list leaves `_flush_prose` as a single 300-400+ char run-on. `split_text` matches it as one "sentence" via `\S[^.!?]*$` and hard-splits on spaces at the 280 budget, mid-clause — the documented long-input regime where Chatterbox degrades into gibberish. Repro: 4-bullet list -> one 439-char blob -> 277-char chunk cut at "…duck all other PC audio while the" | "TTS is speaking…". Also fires mid-turn for a punctuation-free paragraph followed by a blank line (`assembler.py:205-209`). Digest path immune (punctuated model prose).

2. **`_emitted` char-offset desync when a markdown pair straddles an emitted sentence** — `assembler.py:236-239` vs `cleaner.py:13,17`. The assembler re-cleans the growing raw paragraph and tracks progress as a char offset into the CLEANED text; `_EMPHASIS`/`_INLINE_CODE` (and `_LINK`/`_BARE_URL`) are not prefix-stable — a closing marker arriving in a later delta shrinks the cleaned prefix and the stale offset slices mid-word. Executed repro: `feed("**Bottom line: it works. ",0,False)` + `feed("Ship it now.**",1,True)` speaks `["**Bottom line: it works.", "hip it now."]` (literal asterisks spoken, "S" chopped). Backtick variant drops chars the same way. Violates the code's own invariant comment at `assembler.py:219-222`; prefix-stability was fixed only for list ordinals (`cleaner.py:26-31`).

3. **Live path never gets `normalize_for_speech`; `clean_markdown` actively mangles snake_case** — `cleaner.py:43-44`. `normalize_for_speech` (unsnake, `->`→"to", `&`→"and", stray-md strip) is called only on the digest path (`daemon.py:1406-1407`). Live prose gets `clean_markdown`, whose `_EMPHASIS` rule eats intraword underscores pairwise ACROSS identifiers: "Renamed handle_message, added _voiced_upto and chatterbox_max_chunk_chars." -> "Renamed handlemessage, added voicedupto and chatterboxmaxchunkchars." Non-word blobs at peak density in closing recap paragraphs. Verifier also fuzz-confirmed that swapping the live path to `normalize_for_speech` is prefix-stable/safe (unsnake-first ordering must be kept; `_SNAKE` also misses leading-underscore identifiers).

4. **`split_text` corrupts intra-token punctuation** — `chatterbox.py:133` (dup at `chatterbox_worker.py:64`). The sentence regex splits at EVERY `.!?` with no following-whitespace requirement and re-joins with an inserted space (`chatterbox.py:157`): "3.14" -> "3. 14", "daemon.py:123" -> "daemon. py:123", "v2.1.3" -> "v2. 1. 3", "e.g." -> "e. g.". A chunk boundary can also land inside such a token, splitting one word across two separately generated utterances. Hits digests through the same path; live coding prose is just far denser in these tokens.

5. **Turn-final unterminated fragment synthesized alone** — `assembler.py:243-254` + `chatterbox.py:133`. The trailing partial sentence/heading ("Done now") is emitted as its own SpeechItem with no terminal punctuation and no downstream normalization appends one; short unterminated input is Chatterbox's worst case (trailing hallucinated audio). Fix at emission/synth level (append "." / merge tiny tails), not in the packer — in the live path `split_text` sees the fragment alone.

6. **Bullet markers and table content-row pipes never stripped** — `cleaner.py:31,34`. No rule removes `- `/`* `/`+ ` bullets on either path (`_STRAY_MD` lacks `-` and is digest-only); table content rows keep their `|` (only the separator row is stripped). Combines with finding 1 into the fused dash-run.

### Important

7. **`_SENTENCE` `$` alternative emits premature "sentences" mid-stream** — `assembler.py:14`. Any delta ending right after `.!?` is emitted as finished: `feed("The cost is 3.",...)` + `feed("5 million dollars total.",final)` speaks two broken utterances; the backtick case additionally drops a char via finding 2's desync. The `$` branch is unnecessary during streaming (final delivery is guaranteed by force-consume + flush).

8. **Arrows and `&` reach the voice raw in live mode** — `cleaner.py:60,71` digest-only. Confirmed unhandled set on the live path: `->`, `=>`, `-->`, `→`, `&`, `✓`, `✗`, `•`. (Em/en dashes and `…` are normalized inside the chatterbox library's own `punc_norm`.)

9. **Inline-code content passed verbatim** — `cleaner.py:13,40`. Paths, `--flags`, `file.py:123` refs, whole shell commands are spoken character-for-character on both paths; digests merely rarely contain them. "Run `pytest tests/test_daemon.py -k summary --maxfail=1`." is a concrete garble trigger.

10. **One chunk failure flips the voice to Kokoro mid-utterance; 60s cooldown keeps the turn's tail there** — `tts.py:493-497`, `chatterbox.py:225,334-349`. Per-chunk timeout (120s) kills the worker; the failed chunk and everything for the next 60s falls back to Kokoro `DEFAULT_VOICE`; the spoken notice fires once per daemon RUN (`daemon.py:1684-1697`), so later swaps are silent. Clean voice change, not gibberish — a separate defect and a plausible alternate perception of "garbled".

11. **Residue classes unhandled** — emoji (both paths), blockquote `> ` (both), `~~strike~~` (live), `***`/`___` hr (live), schemeless URLs (both); stripped heading lines glue into the following sentence within a paragraph after whitespace collapse.

### Minor

12. **`_BARE_URL` eats the sentence-ending period** — `cleaner.py:19` (`\S+` consumes trailing punctuation): "See link Then run the installer." Merges sentences into the flushed tail.
13. **Punctuation-only chunks reach `model.generate`** — `split_text('...') == ['...']`; reachable live (assembler's only filter is `len > 1`). Worker's `or [""]` also makes the empty-parts branch dead code.
14. **Duplicate-index final skips `_consume(force=True)`** — `assembler.py:51`; needs a compound double-message-loss to trigger; one-line hardening (add force-consume before the flush).
15. **Verifier note:** routing live prose through `normalize_for_speech` is prefix-stable and safe (fuzzed every two-delta split point) — with the caveat that findings 2 and 7 are pre-existing instability channels shared by both cleaners.

## Refuted

- **Worker PCM concatenation lacks crossfade** — accurate reading but unreachable: every current request resolves to exactly one `generate()` (daemon-side pre-split ≤ 280 chars; worker re-split is the identical algorithm), so `len(parts)==1` always.
- **turn_done chime before the sub-minqueue tail (cue inversion)** — audible but by design and unrelated: the chime marks the turn-generation boundary, plays via a detached process, and reordering statements would change nothing (TTS synthesis lag dominates).

## Fix wave (issue: live prose speech quality)

- **A.** Share `normalize_for_speech` with the live path; strip + terminate dash/asterisk/plus bullets; `_BARE_URL` trailing-punctuation fix; `_SNAKE` leading underscore. (findings 3, 6, 8, 12)
- **B.** Sentence/newline-split the turn-final flush tail; punctuate unterminated chunks before generate; skip no-word-char chunks. (1, 5, 13)
- **C.** Whitespace-aware sentence terminators in `assembler._SENTENCE` and both `split_text`s. (4, 7)
- **D.** Rework assembler offset tracking so emitted text can never be invalidated by a late closing marker. (2)

Deferred: 9 (inline-code verbalizer), 10 (mid-utterance fallback UX), 11 (residue classes), 14 (redelivery hardening).
