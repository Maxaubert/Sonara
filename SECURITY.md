# Security policy

## Supported versions

Only the latest release gets security fixes. The plugin installs the release named in
`bin/runtime-version`, and the next hook after a plugin update installs it and replaces the old
runtime (see [README.md](README.md#install)). Older releases are not patched.

## Reporting a vulnerability

Please report security problems in private. Do not open a public issue for a security problem.

Use GitHub private vulnerability reporting: open the **Security** tab of
[github.com/Maxaubert/Sonara](https://github.com/Maxaubert/Sonara) and choose **Report a
vulnerability**. You can also go straight to
[the advisory form](https://github.com/Maxaubert/Sonara/security/advisories/new).

Please include:

- the Sonara version (run `/sonara:doctor`; its `version` row shows it),
- your operating system and version,
- steps to reproduce the problem,
- the impact you expect.

## What to expect

Sonara is maintained by one person, so these are goals, not guarantees.

- An acknowledgement within about a week.
- A fix in a new release once the problem is confirmed.
- Credit in the release notes, if you want it.

## Scope

This policy covers the Sonara runtime (`sonarad`, `sonara-hook` and `sonara`), the plugin scripts
in `bin/` and `hooks/`, and the SDKs in `clients/`. The runtime listens on `127.0.0.1` only, and
its connections need an access token kept in `runtime.json`, which only your Windows or macOS
user can read. [PRIVACY.md](PRIVACY.md) lists what Sonara stores on your computer and what, if
anything, leaves it.
