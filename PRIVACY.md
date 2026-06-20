# Sonara — Privacy Policy

_Last updated: 2026-06-05_

Sonara is a macOS accessibility plugin for [Claude Code](https://claude.ai/code) that reads
Claude Code's output aloud so you can work eyes-free. This policy explains exactly what it
does — and does not do — with your data.

## The short version

**Sonara runs entirely on your own Mac. It does not collect, transmit, sell, or share any of
your data.** There are no servers, no accounts, no telemetry, no analytics, no crash
reporting, and no third-party services.

## What Sonara processes

To speak Claude Code's output, Sonara receives the text that Claude Code passes to it through
plugin hooks — assistant prose, the options in multiple-choice questions, plan text, and
permission-prompt actions. That text is:

- processed **in memory** on your machine,
- handed to the built-in macOS `say` command to be spoken, and
- **not stored and not sent anywhere.**

Sonara's components talk to each other only over a **local socket** on your machine. Nothing
Sonara handles ever leaves your computer.

## What Sonara stores on your machine

Sonara keeps a few small local files under `~/.sonara/` (and LaunchAgent files under
`~/Library/LaunchAgents/`):

- `config.json` — your preferences (voice, speech rate, verbosity).
- `keymap.json` and `hotkeyd.resolved.json` — your global-hotkey bindings.
- `install.json` — local file paths and the install timestamp.
- `*.log` — operational/diagnostic output (startup and errors). Sonara is **not designed to
  record your session content** in these logs.

None of these files are transmitted off your machine.

## Optional diagnostic capture (off by default)

For troubleshooting, Sonara has an **opt-in** capture mode that is **disabled unless you
explicitly enable it** by setting the `SONARA_CAPTURE` environment variable to a folder path.
When enabled, it writes the raw hook payloads it receives (which include session content) to
that folder **on your machine**, to help diagnose problems. It is local-only and never
transmitted. Leave `SONARA_CAPTURE` unset to keep it off; delete the folder to remove any
captured files.

## No personal data, no tracking

Sonara does not collect personal information, does not use cookies or identifiers, does not
profile or track usage, and contains no analytics or third-party data processors.

## Removing your data

Run `sonara uninstall` and delete the `~/.sonara/` folder to remove all of Sonara's local
files.

## Changes to this policy

Any changes will be committed to this file in the project repository, with the "Last updated"
date above revised accordingly.

## Contact

Questions about privacy? Open an issue at
<https://github.com/Maxaubert/sonara/issues> or email hakimi.nima1@gmail.com.
