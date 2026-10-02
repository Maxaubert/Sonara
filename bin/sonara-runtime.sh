# Sourced by bin/sonara-hook-launch and bin/sonara (#202): which installed
# runtime serves this plugin version. Shell builtins only (no fork), so a
# hook stays fast.
#
# Upgrades go one way. Claude Code keeps one plugin folder per version, and
# sessions started before a plugin update keep the old one. Their hooks use
# the newest runtime installed in %LOCALAPPDATA%\Sonara\runtime\ that is at
# least bin/runtime-version, so they never download their older release
# again (which would replace the newer runtime, and back).

# sonara_ver_lt A B: version A (major.minor.patch) is older than B.
sonara_ver_lt() {
    local IFS=. i x y
    local -a a=($1) b=($2)
    for i in 0 1 2; do
        x="${a[i]:-0}"
        y="${b[i]:-0}"
        ((10#$x < 10#$y)) && return 0
        ((10#$x > 10#$y)) && return 1
    done
    return 1
}

# sonara_pick_runtime ROOT WANTED FILE: sets sonara_rt to the newest
# version folder in ROOT that holds FILE and is not older than WANTED
# (WANTED itself when it is installed and nothing newer is); empty when
# there is none.
sonara_pick_runtime() {
    local root="$1" wanted="$2" file="$3" d n
    sonara_rt=""
    if ! [[ "$wanted" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
        [ -f "$root/$wanted/$file" ] && sonara_rt="$wanted"
        return 0
    fi
    for d in "$root"/*/; do
        n="${d%/}"
        n="${n##*/}"
        [[ "$n" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || continue
        [ -f "$root/$n/$file" ] || continue
        sonara_ver_lt "$n" "$wanted" && continue
        if [ -z "$sonara_rt" ] || sonara_ver_lt "$sonara_rt" "$n"; then
            sonara_rt="$n"
        fi
    done
}
