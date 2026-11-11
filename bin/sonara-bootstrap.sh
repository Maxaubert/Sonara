#!/bin/bash
# Sonara runtime bootstrap for macOS (#202): the counterpart of
# bin/sonara-bootstrap.ps1. Needs only what macOS ships: /bin/bash 3.2,
# curl, shasum, tar and codesign.
#
#   sonara-bootstrap.sh --version <v> [--lock <folder>] [--no-start]
#
# Installs the runtime the plugin needs into
# ~/Library/Application Support/Sonara/runtime/<v>/ :
#   1. download sonara-runtime-macos-arm64-<v>.tar.gz and SHA256SUMS from
#      the v<v> GitHub release (SONARA_RELEASE_BASE_URL overrides the base
#      URL, for tests; only then are http:// and file:// allowed),
#   2. check the archive's SHA-256 against SHA256SUMS (a mismatch installs
#      nothing),
#   3. extract it into a staging folder next to the destination, clear the
#      quarantine attribute and check every executable's code signature
#      (codesign --verify --strict; a bad one installs nothing),
#   4. move it into place in one rename (a half-extracted runtime is never
#      seen),
#   5. start it (`sonara start`, unless --no-start or
#      SONARA_BOOTSTRAP_START=0), which replaces a runtime of an older
#      release that is still running,
#   6. remove the folders of older releases (newer ones stay: upgrades go
#      one way; with --no-start all stay, as their runtime may still run).
# bin/sonara-hook-launch runs it in the background from a hook (with
# --lock, the lock folder it took, removed here when done); bin/sonara runs
# it in the foreground before a slash command. A failure is logged to
# <home>/logs/bootstrap.log and recorded in runtime/.bootstrap.failed, so
# hooks retry only after a few minutes.

ver=""
lock=""
start=1
while [ $# -gt 0 ]; do
    case "$1" in
    --version) ver="$2"; shift 2 ;;
    --lock) lock="$2"; shift 2 ;;
    --no-start) start=""; shift ;;
    *) echo "sonara-bootstrap.sh: unknown argument $1" >&2; exit 2 ;;
    esac
done
if [ -z "$ver" ] || [ -z "$HOME" ]; then
    echo "sonara-bootstrap.sh: needs --version and HOME" >&2
    [ -n "$lock" ] && rm -rf "$lock"
    exit 2
fi

here="$(cd "${0%/*}" 2>/dev/null && pwd)" || here="${0%/*}"
base_url="https://github.com/Maxaubert/Sonara/releases/download"
protos="=https"
if [ -n "$SONARA_RELEASE_BASE_URL" ]; then
    base_url="${SONARA_RELEASE_BASE_URL%/}"
    protos="=https,http,file"
fi
base="$HOME/Library/Application Support/Sonara"
root="$base/runtime"
dest="$root/$ver"
failed="$root/.bootstrap.failed"
home="${SONARA_HOME:-$base}"
log_path="$home/logs/bootstrap.log"
name="sonara-runtime-macos-arm64-$ver.tar.gz"
# The executables and libraries of the archive, each checked by codesign.
signed="sonarad sonara-hook sonara libonnxruntime.dylib"
# Each must be signed with an Apple-issued Developer ID Application
# certificate (Apple's Developer ID requirement), not merely carry an
# intact signature: the linker signs every arm64 binary ad-hoc.
# SONARA_CODESIGN_REQUIREMENT replaces it for tests ("-" checks integrity
# only), and only together with SONARA_RELEASE_BASE_URL (a test mirror):
# a real install from GitHub always needs the Developer ID.
requirement='anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] exists and certificate leaf[field.1.2.840.113635.100.6.1.13] exists and certificate leaf[subject.OU] = "WV7BY8CUM8"'
if [ -n "$SONARA_RELEASE_BASE_URL" ] && [ -n "$SONARA_CODESIGN_REQUIREMENT" ]; then
    requirement="$SONARA_CODESIGN_REQUIREMENT"
fi
staging=""
err=""

log() {
    echo "$1"
    mkdir -p "${log_path%/*}" 2>/dev/null &&
        printf '%s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" >> "$log_path" 2>/dev/null
    return 0
}

# fetch URL FILE: download URL to FILE; sets err and fails on any error.
fetch() {
    local code
    code="$(curl -sSL --proto "$protos" --proto-redir "$protos" --connect-timeout 20 --max-time 600 \
        -o "$2" -w '%{http_code}' "$1" 2>/dev/null)"
    local rc=$?
    if [ "$code" = 404 ]; then
        err="no macOS build for $ver ($1 is not there)"
        return 1
    fi
    if [ $rc -ne 0 ]; then
        err="cannot download $1 (curl exit $rc)"
        return 1
    fi
    case "$code" in
    200 | 000) return 0 ;;
    *) err="cannot download $1 (HTTP $code)"; return 1 ;;
    esac
}

# expected SUMS FILE: the SHA-256 that SUMS lists for FILE (lower case).
expected() {
    awk -v n="$2" '{
        f = $2; sub(/^\*/, "", f)
        if (f == n && length($1) == 64 && $1 ~ /^[0-9a-fA-F]+$/) { print tolower($1); exit }
    }' "$1"
}

install_runtime() {
    mkdir -p "$root" || { err="cannot create $root"; return 1; }
    staging="$(mktemp -d "$root/.staging-XXXXXXXX")" || { err="cannot create a staging folder"; return 1; }
    local tgz="$staging/$name" sums="$staging/SHA256SUMS" url="$base_url/v$ver"
    log "Downloading $url/$name"
    fetch "$url/SHA256SUMS" "$sums" || return 1
    fetch "$url/$name" "$tgz" || return 1
    local want got
    want="$(expected "$sums" "$name")"
    [ -n "$want" ] || { err="SHA256SUMS of v$ver lists no $name"; return 1; }
    got="$(shasum -a 256 "$tgz" | awk '{print tolower($1)}')"
    if [ "$got" != "$want" ]; then
        err="checksum mismatch for $name: expected $want, got $got"
        return 1
    fi
    mkdir "$staging/unpacked" || { err="cannot create the unpack folder"; return 1; }
    tar -xzf "$tgz" -C "$staging/unpacked" 2>/dev/null || { err="cannot extract $name"; return 1; }
    local hook dir f
    hook="$(find "$staging/unpacked" -type f -name sonara-hook | head -n 1)"
    [ -n "$hook" ] || { err="$name holds no sonara-hook"; return 1; }
    dir="${hook%/*}"
    # curl sets no quarantine attribute; a copy made by hand might have it.
    xattr -dr com.apple.quarantine "$dir" 2>/dev/null
    for f in $signed; do
        [ -e "$dir/$f" ] || continue
        if ! codesign --verify --strict "$dir/$f" 2>/dev/null; then
            err="the code signature of $f in $name does not verify"
            return 1
        fi
        if [ "$requirement" != - ] && ! codesign --verify --strict -R="$requirement" "$dir/$f" 2>/dev/null; then
            err="$f in $name is not signed with a Developer ID"
            return 1
        fi
    done
    for f in sonarad sonara-hook sonara; do
        [ -x "$dir/$f" ] || { err="$name holds no executable $f"; return 1; }
    done
    # Another bootstrap may have installed it meanwhile, and started it:
    # that install stays.
    if [ -f "$dest/sonara-hook" ]; then
        log "The Sonara runtime $ver was installed meanwhile; keeping it"
        return 0
    fi
    if [ -e "$dest" ]; then
        # A broken earlier install: renamed aside in one step, never
        # deleted file by file in place.
        local aside
        aside="$(mktemp -d "$root/.broken-XXXXXXXX")" &&
            mv "$dest" "$aside/" && rm -rf "$aside"
    fi
    mv "$dir" "$dest" || { err="cannot move the runtime into $dest"; return 1; }
    log "Installed the Sonara runtime $ver in $dest"
    return 0
}

# shellcheck disable=SC2329 # called through the EXIT trap
cleanup() {
    [ -n "$staging" ] && rm -rf "$staging"
    [ -n "$lock" ] && rm -rf "$lock"
}
trap cleanup EXIT

code=0
ok=1
if [ ! -f "$dest/sonara-hook" ]; then
    install_runtime || ok=""
fi
if [ -n "$ok" ]; then
    rm -f "$failed"
    # With --no-start nothing replaces a runtime still running from an
    # older folder, so the older folders stay (`sonara start` removes them
    # once it has replaced that runtime). SONARA_BOOTSTRAP_START=0 (tests)
    # skips only the start.
    started="$start"
    if [ -n "$start" ] && [ "$SONARA_BOOTSTRAP_START" != 0 ]; then
        out="$("$dest/sonara" start 2>&1)"
        rc=$?
        while IFS= read -r line; do
            [ -n "$line" ] && log "$line"
        done <<EOF
$out
EOF
        if [ $rc -ne 0 ]; then
            log "sonara start exited with $rc"
            started=""
        fi
    fi
    # Remove older releases once this one runs (a folder still in use stays
    # and the next `sonara start` removes it). Upgrades go one way: a newer
    # release's folder stays.
    if [ -n "$started" ] && . "$here/sonara-runtime.sh" 2>/dev/null; then
        for d in "$root"/*/; do
            n="${d%/}"
            n="${n##*/}"
            [[ "$n" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || continue
            [[ "$ver" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || continue
            sonara_ver_lt "$n" "$ver" || continue
            if rm -rf "${root:?}/$n" 2>/dev/null && [ ! -e "$root/$n" ]; then
                log "Removed the old runtime $n"
            else
                log "Could not remove the old runtime $n yet"
            fi
        done
    fi
else
    code=1
    log "Sonara runtime $ver not installed: $err"
    mkdir -p "$root" 2>/dev/null &&
        printf '%s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$err" > "$failed" 2>/dev/null
fi
exit $code
