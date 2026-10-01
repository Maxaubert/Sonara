# Codex summarizer smoke test (live, 2026-07-15)

Verified against codex-cli 0.144.3 on the user's machine (ChatGPT account, npm shim
`codex.CMD`; `shutil.which("codex")` resolves it, same pattern as the claude shim).
Adjust `sonara/summarizer.py` if the CLI's flags or model names change.

## Pinned invocation

```
codex exec --sandbox read-only --skip-git-repo-check --color never
      -c mcp_servers={} --disable memories -c model_reasoning_effort="low"
      -m <model> -
```

- Prompt on **stdin** (the trailing `-`), exactly like the claude engine.
- The digest arrives on **stdout with nothing else**; activity/thinking goes to
  stderr. No `--output-last-message` file needed.
- `--sandbox read-only` + `--skip-git-repo-check` + cwd=user-home: no writes, no
  repo access.
- `-c mcp_servers={}` and `--disable memories` keep the user's configured MCP
  servers, plugins and memory writes out of the throwaway call (the user's
  config.toml has node_repl/context7 MCP servers and memories enabled; without
  the overrides every digest would boot them).
- `-c model_reasoning_effort="low"`: the user's config sets high; a digest does
  not need it and low keeps latency at 4-12 s.
- Non-zero exit + JSON error line on stderr for unsupported models -> maps to
  the existing None path.

## Models accepted by this account

Verified against the Codex TUI model picker (2026-07-15 follow-up): the picker
offers sol/terra/luna in the 5.6 line plus 5.5, 5.4 and 5.4-mini, and every
one of them probes OK through the pinned argv.

| Model | Result |
|---|---|
| `gpt-5.6-sol` | OK (account default; 5-12 s) |
| `gpt-5.6-terra` | OK (5 s) |
| `gpt-5.6-luna` | OK (4 s, the fast 5.6) |
| `gpt-5.5` | OK (4 s) |
| `gpt-5.4` | OK (4 s) |
| `gpt-5.4-mini` | OK (4 s, small + fast) |
| `gpt-5.6`, `gpt-5.6-codex`, `gpt-5.6-mini`, `gpt-5.6-sol-mini`, `gpt-5.6-sol-max` | 400 unsupported |
| `gpt-5.5-mini`, `gpt-5.5-sol`, `gpt-5.5-codex`, `gpt-5.5-codex-mini` | 400 unsupported |
| whole `gpt-5.1`/`gpt-5.2` families | 400 "not supported when using Codex with a ChatGPT account" |

UI model list for the Codex engine (fast-first): `gpt-5.6-luna`, `gpt-5.4-mini`,
`gpt-5.6-terra`, `gpt-5.6-sol`, `gpt-5.5`, `gpt-5.4`; engine-switch default
`gpt-5.6-luna` (a summarizer wants the fast tier).

## Digest sanity run

`gpt-5.6-sol`, full candidate argv, summarizer-style prompt with a question
inside the message: exit 0 in 5 s, output restated the question without
answering it ("...and I'm asking whether to deploy to staging."). Firewall
behavior holds.
