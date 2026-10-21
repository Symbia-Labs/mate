#!/bin/sh
# symbia-apply: put a new package on this machine, but only while the bot is between games.
#
#   symbia-apply setup                          Install this script as /usr/local/sbin/symbia-apply and start the timer
#                                               that retries a waiting package every minute.
#   symbia-apply stage FILE SHA256 [options]    Check a package file and, if it is sound, apply it.
#   symbia-apply fetch SHA256 [options]         The same, for a package that has to be downloaded first. The signed link
#                                               is read from standard input, never from the command line.
#   symbia-apply run [--wait SECONDS]           Apply the waiting package, if any. This is what the timer runs.
#   symbia-apply status                         What is installed, what is waiting, and the recent log.
#
#   options: --wait SECONDS   keep checking for that long if a game is in progress (default: check once)
#            --force          accept a release that was rejected before, or one that is already installed
#
# Exit status: 0 applied or nothing to do, 4 checked and still waiting for the bot to be idle, 1 anything else.
#
# Rules, in the order they matter:
#   1. A game in progress is never interrupted. The bot is stopped only when Lichess says it has no game, and "could not
#      ask" counts as "has a game".
#   2. Nothing is installed unless the package matches the checksum it was sent with, every file in it matches its own
#      SHA256SUMS, and the archive holds only paths under symbia-chess/.
#   3. Before installing, the files that will be replaced are saved. If the bot does not reach Lichess within
#      SYMBIA_HEALTH_WAIT seconds, they are put back and the release is marked rejected so that it is not tried again.
#   4. The token is read only to ask Lichess about games. It is never printed, and neither is the signed link.
#   5. A service that was not running when the package arrived is installed but not started.
#
# Every setting below can be overridden from the environment, which is how tests/apply_test.sh runs it in a sandbox.
set -eu
umask 077

STATE=${SYMBIA_STATE:-/var/lib/symbia}
PREFIX=${SYMBIA_PREFIX:-/opt/symbia-chess}
ENV_FILE=${SYMBIA_ENV_FILE:-/etc/symbia/bot.env}
UNIT=${SYMBIA_UNIT:-symbia-bot}
BIN_DIR=${SYMBIA_BIN:-/usr/local/sbin}
UNIT_DIR=${SYMBIA_UNIT_DIR:-/etc/systemd/system}
LICHESS=${SYMBIA_LICHESS:-https://lichess.org}
HEALTH_WAIT=${SYMBIA_HEALTH_WAIT:-90}
STOP_WAIT=${SYMBIA_STOP_WAIT:-90}
POLL=${SYMBIA_POLL:-10}
LOG=$STATE/apply.log

BOT=unknown
WAIT=0
FORCE=0

now() { date -u +%Y-%m-%dT%H:%M:%SZ; }
log() {
    line="$(now) $*"
    printf '%s\n' "$line"
    if [ -d "$STATE" ]; then printf '%s\n' "$line" >> "$LOG"; fi
}
die() { log "error: $*"; exit 1; }

parse_options() {
    WAIT=0
    FORCE=0
    while [ $# -gt 0 ]; do
        case $1 in
            --wait) [ $# -ge 2 ] || die "--wait needs a number of seconds"; WAIT=$2; shift 2 ;;
            --force) FORCE=1; shift ;;
            *) die "unknown option: $1" ;;
        esac
    done
    case $WAIT in ''|*[!0-9]*) die "--wait needs a whole number of seconds" ;; esac
}

prepare_state() { mkdir -p "$STATE"; chmod 700 "$STATE"; }
take_lock() { prepare_state; exec 9> "$STATE/lock"; flock -w 600 9 || die "another symbia-apply is running"; }

# What the bot is doing, from Lichess's point of view: stopped, idle, busy or unknown.
# The token is passed to curl through its config on standard input, so it is never in a command line.
bot_state() {
    svc=$(systemctl is-active "$UNIT" 2>/dev/null || true)
    if [ "$svc" != active ]; then echo stopped; return 0; fi
    tok=$(sed -n 's/^LICHESS_BOT_TOKEN=//p' "$ENV_FILE" 2>/dev/null | head -n 1 || true)
    if [ -z "$tok" ]; then echo unknown; return 0; fi
    reply=$(printf 'header = "Authorization: Bearer %s"\nuser-agent = "symbia-chess-apply"\n' "$tok" |
        curl -fsS -m 20 -K - "$LICHESS/api/account/playing" 2>/dev/null) || { tok=; echo unknown; return 0; }
    tok=
    count=$(printf '%s' "$reply" | python3 -c 'import sys, json; print(len(json.load(sys.stdin)["nowPlaying"]))' 2>/dev/null) || { echo unknown; return 0; }
    case $count in
        0) echo idle ;;
        ''|*[!0-9]*) echo unknown ;;
        *) echo busy ;;
    esac
}

# Sets BOT and returns 0 as soon as the bot is idle or stopped; returns 1 when $1 seconds have passed without that.
wait_idle() {
    end=$(( $(date +%s) + $1 ))
    while :; do
        BOT=$(bot_state)
        case $BOT in idle|stopped) return 0 ;; esac
        if [ "$(date +%s)" -ge "$end" ]; then return 1; fi
        sleep "$POLL"
    done
}

stop_service() {
    if ! timeout "$STOP_WAIT" systemctl stop "$UNIT"; then
        log "the service did not stop within ${STOP_WAIT}s; killing it (no game is running)"
        systemctl kill --signal=SIGKILL "$UNIT" || true
        systemctl stop "$UNIT" || true
    fi
}

# True once the bot has said it is connected to Lichess since the epoch second in $1.
healthy() {
    end=$(( $(date +%s) + HEALTH_WAIT ))
    while :; do
        if [ "$(systemctl is-active "$UNIT" 2>/dev/null || true)" = active ] &&
           journalctl -u "$UNIT" --since "@$1" --no-pager -o cat 2>/dev/null | grep -q 'awaiting challenges'; then
            return 0
        fi
        if [ "$(date +%s)" -ge "$end" ]; then return 1; fi
        sleep "$POLL"
    done
}

set_status() {  # result release
    {
        printf 'installed=%s\n' "$(cat "$PREFIX/RELEASE" 2>/dev/null || echo unknown)"
        printf 'last_result=%s\nlast_release=%s\nat=%s\n' "$1" "$2" "$(now)"
    } > "$STATE/status.tmp"
    mv -f "$STATE/status.tmp" "$STATE/status"
}

# The files an install replaces, saved so that a failed one can be undone. Game records (bot/games) are not touched.
snapshot() {
    rm -rf "$STATE/previous"
    mkdir -p "$STATE/previous" || return 1
    items=
    for p in target bot tools tests RELEASE VERSION .tools/lichess-bot/homemade.py; do
        if [ -e "$PREFIX/$p" ]; then items="$items $p"; fi
    done
    # shellcheck disable=SC2086
    (cd "$PREFIX" && tar -cf "$STATE/previous/files.tar" --exclude=bot/games $items) || return 1
    if [ -f "$UNIT_DIR/$UNIT.service" ]; then cp "$UNIT_DIR/$UNIT.service" "$STATE/previous/unit" || return 1; fi
    return 0
}

restore() {
    [ -f "$STATE/previous/files.tar" ] || return 1
    rm -rf "$PREFIX/target" "$PREFIX/tools" "$PREFIX/tests"
    # A machine installed before this updater existed has no RELEASE or VERSION to put back; the failed package's
    # copies must not stay behind and claim a release that is not installed.
    rm -f "$PREFIX/RELEASE" "$PREFIX/VERSION"
    if [ -d "$PREFIX/bot" ]; then
        find "$PREFIX/bot" -mindepth 1 -maxdepth 1 ! -name games -exec rm -rf {} + || return 1
    fi
    tar -C "$PREFIX" -xpf "$STATE/previous/files.tar" || return 1
    if [ -f "$STATE/previous/unit" ]; then
        cp "$STATE/previous/unit" "$UNIT_DIR/$UNIT.service" || return 1
        systemctl daemon-reload || true
    fi
    return 0
}

run_installer() {
    out=$STATE/install.out
    if (cd "$STATE/pending/pkg/symbia-chess" && PREFIX="$PREFIX" ./deploy/install.sh) > "$out" 2>&1; then
        sed 's/^/    /' "$out" >> "$LOG"
        return 0
    fi
    tail -n 15 "$out" | sed 's/^/    /' | tee -a "$LOG"
    return 1
}

roll_back() {  # release was_running
    mkdir -p "$STATE/rejected"
    : > "$STATE/rejected/$1"
    if [ "$2" = 1 ]; then stop_service; fi
    if restore; then log "previous files restored"; else log "the previous files could not be restored"; fi
    rm -rf "$STATE/pending"
    if [ "$2" = 1 ]; then
        started=$(date +%s)
        systemctl start "$UNIT" || true
        if healthy "$started"; then log "the previous release is running again"; else log "the previous release did not come back either; read the journal"; fi
    fi
    set_status rolled-back "$1"
    log "release $1 is marked rejected and will not be tried again unless it is sent with --force"
}

# Apply what is in $STATE/pending, if the bot allows it.
do_run() {
    [ -f "$STATE/pending/release" ] || return 0
    rel=$(cat "$STATE/pending/release")
    if ! wait_idle "$WAIT"; then
        if [ ! -f "$STATE/pending/noted" ]; then
            log "release $rel is waiting. The bot is $BOT, so nothing was stopped; it will be applied between games."
            : > "$STATE/pending/noted"
        fi
        set_status waiting "$rel"
        return 4
    fi
    log "release $rel: the bot is $BOT; applying"
    case $(systemctl is-active "$UNIT" 2>/dev/null || true) in active|activating|reloading) running=1 ;; *) running=0 ;; esac
    if [ "$running" = 1 ]; then stop_service; fi
    if ! snapshot; then
        log "the current files could not be saved, so nothing was installed"
        if [ "$running" = 1 ]; then systemctl start "$UNIT" || true; fi
        set_status error "$rel"
        return 1
    fi
    if ! run_installer; then
        log "the installer failed; putting the previous files back"
        roll_back "$rel" "$running"
        return 1
    fi
    if [ "$running" = 1 ]; then
        started=$(date +%s)
        systemctl start "$UNIT" || true
        if ! healthy "$started"; then
            log "the bot did not reach Lichess within ${HEALTH_WAIT}s; putting the previous files back"
            roll_back "$rel" "$running"
            return 1
        fi
        log "the bot is connected to Lichess"
    else
        log "the service was not running, so it was left stopped"
    fi
    rm -rf "$STATE/pending"
    set_status ok "$rel"
    log "release $rel is installed"
    return 0
}

# Check a package file and move it to $STATE/pending. $1 file, $2 expected sha256.
do_stage() {
    file=$1
    want=$2
    case $want in *[!0-9a-f]*|'') die "the checksum must be 64 lowercase hex characters" ;; esac
    [ "${#want}" -eq 64 ] || die "the checksum must be 64 lowercase hex characters"
    [ -f "$file" ] || die "no such file: $file"
    got=$(sha256sum "$file" | cut -c1-64)
    [ "$got" = "$want" ] || die "the package does not match the checksum it was sent with"

    list=$STATE/list.txt
    tar -tzf "$file" > "$list" 2>/dev/null || die "the package is not a readable archive"
    [ -s "$list" ] || die "the package is empty"
    if grep -v -e '^symbia-chess$' -e '^symbia-chess/' "$list" | grep -q .; then die "the package has paths outside symbia-chess/"; fi
    if grep -q -e '^\.\./' -e '/\.\./' -e '/\.\.$' "$list"; then die "the package has a path with .. in it"; fi
    if tar -tvzf "$file" 2>/dev/null | cut -c1 | grep -q '[lhbcp]'; then die "the package has links or special files"; fi

    work=$STATE/staging
    rm -rf "$work"
    mkdir -p "$work"
    tar -xzf "$file" -C "$work" --no-same-owner || { rm -rf "$work"; die "the package could not be unpacked"; }
    top=$work/symbia-chess
    for need in SHA256SUMS RELEASE bin/uci deploy/install.sh deploy/apply.sh; do
        [ -f "$top/$need" ] || { rm -rf "$work"; die "the package has no $need"; }
    done
    (cd "$top" && sha256sum --quiet -c SHA256SUMS > /dev/null 2>&1) || { rm -rf "$work"; die "a file in the package does not match the package's own SHA256SUMS"; }
    rel=$(cat "$top/RELEASE")
    case $rel in *[!0-9a-f]*|'') rm -rf "$work"; die "the package's RELEASE is malformed" ;; esac
    [ "${#rel}" -eq 12 ] || { rm -rf "$work"; die "the package's RELEASE is malformed"; }
    calc=$(sha256sum "$top/SHA256SUMS" | cut -c1-12)
    [ "$calc" = "$rel" ] || { rm -rf "$work"; die "the package's RELEASE does not match its SHA256SUMS"; }

    cur=$(cat "$PREFIX/RELEASE" 2>/dev/null || true)
    if [ "$rel" = "$cur" ] && [ "$FORCE" = 0 ]; then
        rm -rf "$work" "$STATE/pending"
        log "release $rel is already installed; nothing to do"
        return 0
    fi
    if [ -e "$STATE/rejected/$rel" ]; then
        if [ "$FORCE" = 0 ]; then rm -rf "$work"; die "release $rel was rejected earlier; send it with --force to try it again"; fi
        rm -f "$STATE/rejected/$rel"
    fi
    rm -rf "$STATE/pending"
    mkdir -p "$STATE/pending"
    mv "$work" "$STATE/pending/pkg"
    printf '%s\n' "$rel" > "$STATE/pending/release"
    log "release $rel is staged (package checksum ok, files ok)"
    return 0
}

finish() {  # exit with do_run's status
    rc=0
    do_run || rc=$?
    exit "$rc"
}

cmd_stage() {
    [ $# -ge 2 ] || die "usage: symbia-apply stage FILE SHA256 [--wait SECONDS] [--force]"
    file=$1
    want=$2
    shift 2
    parse_options "$@"
    take_lock
    do_stage "$file" "$want"
    finish
}

cmd_fetch() {
    [ $# -ge 1 ] || die "usage: symbia-apply fetch SHA256 [--wait SECONDS] [--force]   (signed link on standard input)"
    want=$1
    shift
    parse_options "$@"
    take_lock
    link=
    IFS= read -r link || true
    [ -n "$link" ] || die "no link on standard input"
    mkdir -p "$STATE/incoming"
    pkgfile=$STATE/incoming/package.tar.gz
    rm -f "$pkgfile"
    trap 'rm -f "$STATE/incoming/package.tar.gz" "$STATE/curl.err"' EXIT
    log "downloading the package"
    if ! printf 'url = "%s"\n' "$link" | curl -K - -fsS -m 300 -o "$pkgfile" 2> "$STATE/curl.err"; then
        link=
        sed 's#[A-Za-z][A-Za-z0-9+.-]*://[^ ]*#<link>#g' "$STATE/curl.err" | tee -a "$LOG"
        rm -f "$pkgfile" "$STATE/curl.err"
        die "the download failed"
    fi
    link=
    rm -f "$STATE/curl.err"
    log "downloaded $(wc -c < "$pkgfile" | tr -d ' ') bytes"
    do_stage "$pkgfile" "$want" || { rm -f "$pkgfile"; exit 1; }
    rm -f "$pkgfile"
    finish
}

cmd_run() {
    parse_options "$@"
    prepare_state
    exec 9> "$STATE/lock"
    flock -n 9 || exit 0
    finish
}

cmd_status() {
    echo "installed release: $(cat "$PREFIX/RELEASE" 2>/dev/null || echo 'unknown (installed before B10)')"
    if [ -f "$STATE/pending/release" ]; then echo "waiting release:   $(cat "$STATE/pending/release")"; else echo "waiting release:   none"; fi
    if [ -d "$STATE/rejected" ] && [ -n "$(ls "$STATE/rejected" 2>/dev/null)" ]; then echo "rejected:          $(ls "$STATE/rejected" | tr '\n' ' ')"; fi
    echo "service:           $(systemctl is-active "$UNIT" 2>/dev/null || true), $(systemctl is-enabled "$UNIT" 2>/dev/null || true)"
    echo "bot (per Lichess): $(bot_state)"
    if [ -f "$STATE/status" ]; then echo "last apply:        $(tr '\n' ' ' < "$STATE/status")"; fi
    echo "-- apply log"
    if [ -f "$LOG" ]; then tail -n 15 "$LOG"; else echo "(none)"; fi
    echo "-- journal"
    journalctl -u "$UNIT" -n 12 --no-pager -o cat 2>/dev/null | cut -c1-200 || true
}

cmd_setup() {
    src=$0
    case $src in /*) ;; *) src=$(cd "$(dirname "$src")" && pwd)/$(basename "$src") ;; esac
    prepare_state
    mkdir -p "$BIN_DIR"
    if [ "$src" != "$BIN_DIR/symbia-apply" ]; then
        cp "$src" "$BIN_DIR/.symbia-apply.new"
        chmod 755 "$BIN_DIR/.symbia-apply.new"
        mv -f "$BIN_DIR/.symbia-apply.new" "$BIN_DIR/symbia-apply"
    fi
    if [ -n "${SYMBIA_NO_SYSTEMD:-}" ]; then
        log "symbia-apply installed in $BIN_DIR (systemd left alone)"
        return 0
    fi
    mkdir -p "$UNIT_DIR"
    cat > "$UNIT_DIR/symbia-apply.service" <<EOF
[Unit]
Description=Apply a waiting Symbia chess package when the bot is between games
After=network-online.target

[Service]
Type=oneshot
ExecStart=$BIN_DIR/symbia-apply run
EOF
    cat > "$UNIT_DIR/symbia-apply.timer" <<EOF
[Unit]
Description=Look for a waiting Symbia chess package every minute

[Timer]
OnActiveSec=60s
OnUnitActiveSec=60s
AccuracySec=5s

[Install]
WantedBy=timers.target
EOF
    chmod 644 "$UNIT_DIR/symbia-apply.service" "$UNIT_DIR/symbia-apply.timer"
    systemctl daemon-reload
    systemctl enable --now symbia-apply.timer > /dev/null 2>&1
    log "symbia-apply installed in $BIN_DIR; timer enabled"
}

cmd=${1:-}
[ $# -gt 0 ] && shift
case $cmd in
    setup) cmd_setup "$@" ;;
    stage) cmd_stage "$@" ;;
    fetch) cmd_fetch "$@" ;;
    run) cmd_run "$@" ;;
    status) cmd_status "$@" ;;
    *) sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'; exit 1 ;;
esac
