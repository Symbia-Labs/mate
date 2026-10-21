#!/bin/sh
# Runs the engine behind a Lichess bot account with lichess-bot.
# Usage: tools/bot.sh
# Reads the account's API token without echoing it (or takes $LICHESS_BOT_TOKEN), offers to turn a fresh account
# into a bot account, then plays the challenges that bot/config.yml accepts and keeps every game in bot/games/.
# The token needs the "Play games with the bot API" scope. Ctrl-C stops the bot.
set -eu
here=$(cd "$(dirname "$0")/.." && pwd)
bot="$here/.tools/lichess-bot"
py="$bot/.venv/bin/python"
[ -x "$here/target/release/uci" ] || { echo "No engine yet. Run: cargo build --release" >&2; exit 1; }
if [ -z "${LICHESS_BOT_TOKEN:-}" ]; then
    printf 'Lichess bot token: '
    stty -echo
    read -r LICHESS_BOT_TOKEN
    stty echo
    printf '\n'
    export LICHESS_BOT_TOKEN
fi
account=$(curl -sS -H "Authorization: Bearer $LICHESS_BOT_TOKEN" https://lichess.org/api/account |
    "$py" -c 'import json, sys; a = json.load(sys.stdin); print(a.get("username") or "?", a.get("title") or "-", a.get("count", {}).get("all", 0))')
set -- $account
name=${1:-}
title=${2:-}
games=${3:-0}
[ "$name" != "?" ] || { echo "Lichess did not accept the token." >&2; exit 1; }
upgrade=
if [ "$title" != "BOT" ]; then
    [ "$games" = 0 ] || { echo "$name has played $games games, so it cannot become a bot account. Use a new account." >&2; exit 1; }
    printf '%s is not a bot account. Make it one? This cannot be undone. [y/N] ' "$name"
    read -r answer
    [ "$answer" = y ] || exit 1
    upgrade=-u
fi
mkdir -p "$here/bot/games"
cp "$here/bot/homemade.py" "$bot/homemade.py"
cd "$bot"
exec caffeinate -i "$py" lichess-bot.py $upgrade --config "$here/bot/config.yml" --logfile "$here/bot/lichess-bot.log"
