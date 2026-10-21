#!/bin/sh
# Build the package that deploy/install.sh unpacks, from an engine binary built for Linux.
#
# Usage: tools/package.sh path/to/uci [out_dir]        (out_dir defaults to dist/; needs GNU tar and sha256sum)
#
# The package holds the engine, the bot's own files, the installer, the updater (deploy/apply.sh) and the unit, a
# SHA256SUMS the installer checks, and a RELEASE that names the package.
# It does not hold lichess-bot, which the installer fetches at the commit named in deploy/lichess-bot.pin. This script
# refuses to build if anything in the package names a path on someone's Mac or looks like a Lichess token.
set -eu

here=$(cd "$(dirname "$0")/.." && pwd)
uci=${1:?usage: tools/package.sh path/to/uci [out_dir]}
out=${2:-$here/dist}
[ -f "$uci" ] || { echo "package: no such file: $uci" >&2; exit 1; }

stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
top="$stage/symbia-chess"
mkdir -p "$top/bin" "$top/bot" "$top/tools" "$top/tests/fixtures" "$top/deploy" "$out"

install -m 0755 "$uci" "$top/bin/uci"
install -m 0644 "$here/bot/config.yml" "$here/bot/homemade.py" "$top/bot/"
install -m 0755 "$here/tools/hook_smoke.py" "$here/tools/fingerprint.py" "$top/tools/"
install -m 0644 "$here/tests/fixtures/games_sample.moves" "$top/tests/fixtures/"
install -m 0755 "$here/deploy/install.sh" "$here/deploy/apply.sh" "$top/deploy/"
install -m 0644 "$here/deploy/symbia-bot.service" "$here/deploy/lichess-bot.pin" "$top/deploy/"

engine=$(sha256sum "$top/bin/uci" | cut -c1-64)
{
    echo "engine_sha256=$engine"
    echo "lichess_bot=$(sed -n 's/^PIN_COMMIT=//p' "$top/deploy/lichess-bot.pin")"
    echo "built_with=$(rustc --version 2>/dev/null || echo unknown)"
    echo "built_on=$(uname -sm)"
} > "$top/VERSION"

grep -q '^token: "x*"' "$top/bot/config.yml" || { echo "package: bot/config.yml holds something other than the token placeholder" >&2; exit 1; }
if grep -rIl -e '/Users/' -e 'lip_' "$top" >/dev/null 2>&1; then
    echo "package: these files name a Mac path or look like they hold a token:" >&2
    grep -rIl -e '/Users/' -e 'lip_' "$top" >&2
    exit 1
fi
if grep -a -q -e '/Users/' "$top/bin/uci"; then
    echo "package: the engine binary names a Mac path (the build kept a source path)" >&2
    exit 1
fi

(cd "$top" && find . -type f ! -name SHA256SUMS ! -name RELEASE | sort | xargs sha256sum > SHA256SUMS)

# RELEASE names this exact set of files: the first 12 characters of the SHA-256 of SHA256SUMS. A change to the bot's
# config gives a new name even when the engine binary is the same. symbia-apply checks that it matches.
release=$(sha256sum "$top/SHA256SUMS" | cut -c1-12)
echo "$release" > "$top/RELEASE"

name="symbia-chess-$release-linux-$(uname -m)"
tar --sort=name --owner=0 --group=0 --numeric-owner --mtime='2000-01-01 00:00:00' -C "$stage" -czf "$out/$name.tar.gz" symbia-chess
echo "$out/$name.tar.gz"
ls -l "$out/$name.tar.gz" | awk '{print $5, "bytes"}'
