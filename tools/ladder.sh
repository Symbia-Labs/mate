#!/bin/sh
# Plays the engine against Stockfish 19 at each UCI_Elo level with fastchess, one PGN per level.
# Usage: tools/ladder.sh RUN TC ROUNDS LEVEL...
#   RUN     directory name under matches/
#   TC      fastchess time control, e.g. 10+0.1
#   ROUNDS  openings per level; each is played twice with colors reversed
# Example: tools/ladder.sh sweep 10+0.1 20 1320 1500 1700 1900 2100 2300
set -eu
here=$(cd "$(dirname "$0")/.." && pwd)
run=$1
tc=$2
rounds=$3
shift 3
out="$here/matches/$run"
mkdir -p "$out"
for level in "$@"; do
    "$here/.tools/fastchess-mac-arm64/fastchess" \
        -engine cmd="$here/target/release/uci" name=chess \
        -engine cmd="$here/.tools/stockfish/stockfish-macos-universal" name="sf$level" \
        option.Threads=1 option.Hash=16 option.UCI_LimitStrength=true option.UCI_Elo="$level" \
        -each tc="$tc" \
        -openings file="$here/.tools/noob_3moves.epd" format=epd order=random \
        -srand 20261019 -rounds "$rounds" -repeat -concurrency 8 \
        -resign movecount=4 score=800 \
        -draw movenumber=60 movecount=10 score=5 \
        -event "B4 $run $tc sf$level" \
        -pgnout file="$out/sf$level.pgn" notation=san append=false
done
