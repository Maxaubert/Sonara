---
name: sonara-redeploy
description: Use to try a Sonara branch build in the real Claude Code plugin on this PC, or when the runtime behaves unlike the repo. Safe stop, copy and start into %LOCALAPPDATA%\Sonara\runtime, in the same or a new version folder, and how to roll back.
---

# Sonara safe redeploy

The plugin never runs the repo or `target/`: hooks and commands run the newest folder in
`%LOCALAPPDATA%\Sonara\runtime\<version>\` that is at least the plugin's `bin/runtime-version`
(`bin/sonara-runtime.sh`). Check what runs before diagnosing anything:

```sh
ls "$LOCALAPPDATA/Sonara/runtime/"
"$LOCALAPPDATA/Sonara/runtime/<ver>/sonara.exe" version
```

Rules: never redeploy during someone's live session (ask first), always `stop` before copying
and `start` after, and never touch the home (`config.json`, `logs\`, keys) to do it.

## 1. Build

```sh
export PATH="$HOME/.cargo/bin:$PATH"
cargo build -p sonarad -p sonara-hook -p sonara-cli --release
python packaging/runtime_dlls.py stage target/release
```

## 2a. Same version folder (the branch keeps the installed version)

```sh
R="$LOCALAPPDATA/Sonara/runtime/<ver>"
"$R/sonara.exe" stop     # writes <home>\stopped, restores ducked apps, waits for the exit
cp target/release/{sonarad,sonara-hook,sonara}.exe "$R/"
cp target/release/{onnxruntime.dll,onnxruntime-LICENSE.txt,onnxruntime-ThirdPartyNotices.txt,msvcp140.dll,msvcp140_1.dll,vcruntime140.dll,vcruntime140_1.dll} "$R/"
"$R/sonara.exe" start    # clears stopped
```

## 2b. New version folder (the branch bumped the version, the usual case)

The installed plugin picks the newer folder up at the next hook, so no plugin change is needed.
Lay it out exactly like a release by unpacking the release zip:

```sh
OLD="$LOCALAPPDATA/Sonara/runtime/<installed ver>"
NEW_VER="$(cat bin/runtime-version)"
python packaging/release_zip.py --out "$TMP/sonara-zip"     # sonara-runtime-win-x64-<NEW_VER>.zip
"$OLD/sonara.exe" stop
python -c "import zipfile,sys; zipfile.ZipFile(sys.argv[1]).extractall(sys.argv[2])" \
  "$TMP/sonara-zip/sonara-runtime-win-x64-$NEW_VER.zip" "$TMP/sonara-zip/x"
mv "$TMP/sonara-zip/x/sonara-runtime-win-x64-$NEW_VER" "$LOCALAPPDATA/Sonara/runtime/$NEW_VER"
"$LOCALAPPDATA/Sonara/runtime/$NEW_VER/sonara.exe" start
```

`start` of the new folder shuts down and REMOVES the folders of older releases. That is fine:
`/sonara:start` reinstalls the plugin's release when its folder is missing.

## 3. Verify

`"$LOCALAPPDATA/Sonara/runtime/<ver>/sonara.exe" version` and `/sonara:doctor` (its `version`
row) show the branch version; `logs\sonarad.log` shows the new process starting.

## Roll back, and after the merge

A branch folder named like the coming release blocks the real release: the launcher sees the
version installed and never downloads it. After testing, or once the PR is merged:

```sh
"$LOCALAPPDATA/Sonara/runtime/<branch ver>/sonara.exe" stop
trash "$(cygpath -w "$LOCALAPPDATA/Sonara/runtime/<branch ver>")"
```

Then run `/sonara:start`: it installs the release named in the plugin's `bin/runtime-version`
and clears `<home>\stopped` (hooks stay quiet while that file exists).
