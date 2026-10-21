#!/bin/sh
# Install the Symbia chess bot from an unpacked package (see tools/package.sh).
#
#   sudo deploy/install.sh
#       Installs under /opt/symbia-chess and sets up the symbia-bot service and the symbia-apply updater (deploy/apply.sh).
#       It does not start the service, because the Lichess token may not be there yet; the last lines it prints say how
#       to add it.
#
#   PREFIX=/some/dir SYMBIA_NO_SYSTEMD=1 deploy/install.sh
#       Installs anywhere, as any user, and leaves systemd alone. For testing. SYMBIA_UNIT_DIR=/dir also writes the
#       unit file there, so that `systemd-analyze verify` can read it.
#
# The installed tree has the same shape as the repository (target/release/uci, bot/, .tools/lichess-bot/), which is
# why one bot/config.yml works in both places. Running it again updates the engine and the bot files in place; on a
# running machine that is done by symbia-apply, which also decides when it is safe to restart the bot.
set -eu

pkg=$(cd "$(dirname "$0")/.." && pwd)
prefix=${PREFIX:-/opt/symbia-chess}
unit_dir=${SYMBIA_UNIT_DIR:-/etc/systemd/system}
no_systemd=${SYMBIA_NO_SYSTEMD:-}

say() { printf '%s\n' "$*"; }
die() { printf 'install: %s\n' "$*" >&2; exit 1; }

# shellcheck disable=SC1091
. "$pkg/deploy/lichess-bot.pin"

command -v git >/dev/null || die "git is missing. On Ubuntu: apt-get install -y git"
command -v python3 >/dev/null || die "python3 is missing. On Ubuntu: apt-get install -y python3 python3-venv"
python3 -c 'import sys; sys.exit(sys.version_info < (3, 11))' || die "lichess-bot needs Python 3.11 or newer; this is $(python3 -V)"
python3 -c 'import ensurepip, venv' 2>/dev/null || die "the venv module is missing. On Ubuntu: apt-get install -y python3-venv"
[ "$(uname -s)" = Linux ] || die "this package is a Linux build"

if [ -f "$pkg/SHA256SUMS" ]; then
    (cd "$pkg" && sha256sum --quiet -c SHA256SUMS) || die "the package does not match its SHA256SUMS"
    say "package checksums ok"
fi

mkdir -p "$prefix/target/release" "$prefix/bot/games" "$prefix/tools" "$prefix/tests/fixtures" "$prefix/.tools"
install -m 0755 "$pkg/bin/uci" "$prefix/target/release/uci"
install -m 0644 "$pkg/bot/config.yml" "$prefix/bot/config.yml"
install -m 0644 "$pkg/bot/homemade.py" "$prefix/bot/homemade.py"
install -m 0755 "$pkg/tools/hook_smoke.py" "$pkg/tools/fingerprint.py" "$prefix/tools/"
install -m 0644 "$pkg/tests/fixtures/games_sample.moves" "$prefix/tests/fixtures/games_sample.moves"
for f in VERSION RELEASE; do
    if [ -f "$pkg/$f" ]; then install -m 0644 "$pkg/$f" "$prefix/$f"; fi
done

printf 'uci\nquit\n' | "$prefix/target/release/uci" | grep -q '^uciok' || die "the engine did not answer the uci command"
say "engine answers"

lb="$prefix/.tools/lichess-bot"
# After the first install the directory belongs to the symbia user, and git run as root refuses to touch a repository
# it does not own ("dubious ownership"). Naming the directory as safe on the command line is honored by git for this
# one call and changes no configuration.
lbgit() { git -c safe.directory="$lb" -C "$lb" "$@"; }
if [ ! -d "$lb/.git" ]; then
    mkdir -p "$lb"
    lbgit init -q
    lbgit remote add origin "$PIN_REPO"
fi
if [ "$(lbgit rev-parse -q --verify HEAD 2>/dev/null || true)" != "$PIN_COMMIT" ]; then
    lbgit fetch -q --depth 1 origin "$PIN_COMMIT"
    lbgit checkout -q -f FETCH_HEAD
fi
[ "$(lbgit rev-parse HEAD)" = "$PIN_COMMIT" ] || die "lichess-bot is not at $PIN_COMMIT"
say "lichess-bot at $PIN_COMMIT"

[ -x "$lb/.venv/bin/python" ] || python3 -m venv "$lb/.venv"
"$lb/.venv/bin/pip" install --quiet --disable-pip-version-check -r "$lb/requirements.txt"
"$lb/.venv/bin/pip" freeze > "$prefix/bot/requirements.installed.txt"
cp "$prefix/bot/homemade.py" "$lb/homemade.py"
say "python environment ready ($("$lb/.venv/bin/python" -V)); versions in bot/requirements.installed.txt"

if [ -n "$no_systemd" ]; then
    if [ -n "${SYMBIA_UNIT_DIR:-}" ]; then
        mkdir -p "$unit_dir"
        sed "s|@PREFIX@|$prefix|g" "$pkg/deploy/symbia-bot.service" > "$unit_dir/symbia-bot.service"
        say "unit written to $unit_dir/symbia-bot.service"
    fi
    say "installed under $prefix without the service"
    exit 0
fi

[ "$(id -u)" = 0 ] || die "run this as root (sudo) to set up the service"
id symbia >/dev/null 2>&1 || useradd --system --home-dir "$prefix" --shell /usr/sbin/nologin symbia
chown -R symbia:symbia "$prefix/bot" "$lb"
mkdir -p /etc/symbia
if [ ! -e /etc/symbia/bot.env ]; then
    (umask 077 && printf 'LICHESS_BOT_TOKEN=\n' > /etc/symbia/bot.env)
fi
sed "s|@PREFIX@|$prefix|g" "$pkg/deploy/symbia-bot.service" > "$unit_dir/symbia-bot.service"
systemctl daemon-reload
systemctl enable symbia-bot >/dev/null 2>&1
say "service installed and enabled (install.sh never starts it)"
"$pkg/deploy/apply.sh" setup
if grep -q '^LICHESS_BOT_TOKEN=.' /etc/symbia/bot.env; then
    say "the bot token is already in /etc/symbia/bot.env"
else
    say "Next: put the bot token on the one line of /etc/symbia/bot.env (LICHESS_BOT_TOKEN=...), then: systemctl start symbia-bot"
fi
