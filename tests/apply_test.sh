#!/bin/sh
# Tests for deploy/apply.sh and deploy/push.sh, in a sandbox with no systemd, no Lichess and no network beyond loopback.
#
#   dash tests/apply_test.sh        (use dash, or any shell where `sh` is dash: Lightsail runs launch scripts under it)
#
# systemctl, journalctl and the one Lichess call are stand-ins (written below); tar, flock, timeout, sha256sum, python3
# and curl are the real ones. Each case gets its own sandbox: a fake install under opt/, a fake token in bot.env, and a
# fake service whose state is a file. A "package" is built the way tools/package.sh builds one (SHA256SUMS, then a
# RELEASE taken from it), around a fake install.sh whose behavior the case chooses.
set -u

here=$(cd "$(dirname "$0")/.." && pwd)
apply=$here/deploy/apply.sh
push=$here/deploy/push.sh
root=$(mktemp -d)
TOKEN=lip_TESTTOKENabcdef0123456789
SIG=SECRETSIG123456
REAL_CURL=$(command -v curl)
export REAL_CURL
export NO_PROXY=127.0.0.1,localhost no_proxy=127.0.0.1,localhost
http_pid=
cleanup() {
    [ -z "$http_pid" ] || kill "$http_pid" 2>/dev/null
    rm -rf "$root"
}
trap cleanup EXIT

pass=0
fail=0
ok() { pass=$((pass + 1)); printf 'ok   %s\n' "$*"; }
bad() { fail=$((fail + 1)); printf 'FAIL %s\n' "$*"; }
t() { d=$1; shift; if "$@" > /dev/null 2>&1; then ok "$d"; else bad "$d"; fi; }
has() { grep -q -e "$2" "$1" 2> /dev/null; }
lacks() { ! grep -q -e "$2" "$1" 2> /dev/null; }
eq() { [ "$1" = "$2" ]; }
ne() { [ "$1" != "$2" ]; }
exists() { [ -e "$1" ]; }
absent() { [ ! -e "$1" ]; }

# ---- stand-ins --------------------------------------------------------------------------------------------------
mkdir -p "$root/shim"
cat > "$root/shim/systemctl" <<'EOF'
#!/bin/sh
echo "$*" >> "$W/sys.log"
s=$(cat "$W/svc" 2>/dev/null || echo inactive)
case "$1" in
    is-active) echo "$s"; [ "$s" = active ] ;;
    is-enabled) echo enabled ;;
    stop) if [ -f "$W/stop_hangs" ]; then sleep 30; fi; echo inactive > "$W/svc" ;;
    kill) echo inactive > "$W/svc" ;;
    start|restart) if [ -f "$W/start_fails" ]; then echo failed > "$W/svc"; else echo active > "$W/svc"; fi ;;
    *) : ;;
esac
exit 0
EOF
cat > "$root/shim/journalctl" <<'EOF'
#!/bin/sh
echo "$*" >> "$W/journal.log"
s=$(cat "$W/svc" 2>/dev/null || echo inactive)
if [ "$s" = active ] && grep -q -f "$W/welcome_if" "$P/bot/config.yml" 2> /dev/null; then
    echo "You're now connected to https://lichess.org/ and awaiting challenges."
fi
exit 0
EOF
cat > "$root/shim/curl" <<'EOF'
#!/bin/sh
# Answers the Lichess "games in progress" call from files; anything else goes to the real curl.
echo "$*" >> "$W/curl_argv.log"
for last; do :; done
case "$last" in
    */api/account/playing)
        cat > "$W/curl_stdin"
        n=$(( $(cat "$W/curl_calls" 2> /dev/null || echo 0) + 1 ))
        echo "$n" > "$W/curl_calls"
        if [ -f "$W/lichess_down" ]; then echo "curl: (7) Failed to connect" >&2; exit 7; fi
        if [ -f "$W/idle_after" ] && [ "$n" -gt "$(cat "$W/idle_after")" ]; then echo '{"nowPlaying":[]}'; exit 0; fi
        cat "$W/playing.json"
        exit 0
        ;;
esac
exec "$REAL_CURL" "$@"
EOF
chmod +x "$root/shim/"*

# ---- sandbox ----------------------------------------------------------------------------------------------------
new_case() {
    name=$1
    W=$root/$name
    P=$W/opt
    mkdir -p "$P/target/release" "$P/bot/games" "$P/tools" "$P/tests/fixtures" "$P/.tools/lichess-bot" "$W/units" "$W/bin" "$W/pkgs"
    printf 'uci old\n' > "$P/target/release/uci"
    printf 'config old\n' > "$P/bot/config.yml"
    printf 'homemade old\n' > "$P/bot/homemade.py"
    printf 'homemade old\n' > "$P/.tools/lichess-bot/homemade.py"
    printf 'tool old\n' > "$P/tools/fingerprint.py"
    printf 'fixture old\n' > "$P/tests/fixtures/games_sample.moves"
    printf 'game record\n' > "$P/bot/games/game1.pgn"
    printf 'aaaaaaaaaaaa\n' > "$P/RELEASE"
    printf 'version old\n' > "$P/VERSION"
    printf 'unit old\n' > "$W/units/symbia-bot.service"
    printf 'LICHESS_BOT_TOKEN=%s\n' "$TOKEN" > "$W/bot.env"
    printf 'active\n' > "$W/svc"
    printf '{"nowPlaying":[]}\n' > "$W/playing.json"
    printf 'config\n' > "$W/welcome_if"
    export W P
    export SYMBIA_STATE=$W/state SYMBIA_PREFIX=$P SYMBIA_ENV_FILE=$W/bot.env SYMBIA_UNIT_DIR=$W/units SYMBIA_BIN=$W/bin
    export SYMBIA_LICHESS=https://lichess.invalid SYMBIA_HEALTH_WAIT=3 SYMBIA_STOP_WAIT=2 SYMBIA_POLL=1
    unset SYMBIA_NO_SYSTEMD
    before=$(treehash)
}

treehash() {
    (cd "$P" && find . -path ./bot/games -prune -o -type f -print | sort | xargs sha256sum | sha256sum | cut -c1-16)
    sha256sum < "$W/units/symbia-bot.service" | cut -c1-16
}

# A package the way tools/package.sh builds one. mkpkg NAME VARIANT; sets PKG (the file), SHA (its checksum), REL.
#   ok         new config, installs cleanly
#   bad        config text that the health check will not accept after the update ("config bad")
#   failinstall  install.sh copies the new config and then fails
#   tamper     a file is changed after SHA256SUMS was made
mkpkg() {
    n=$1
    variant=$2
    d=$W/pkgs/$n/symbia-chess
    mkdir -p "$d/deploy" "$d/bin" "$d/bot"
    printf 'uci new\n' > "$d/bin/uci"
    case $variant in
        bad) printf 'config bad\n' > "$d/bot/config.yml" ;;
        *) printf 'config new\n' > "$d/bot/config.yml" ;;
    esac
    cp "$apply" "$d/deploy/apply.sh"
    if [ "$variant" = failinstall ]; then : > "$d/fail_install"; fi
    cat > "$d/deploy/install.sh" <<'EOF'
#!/bin/sh
set -e
pkg=$(cd "$(dirname "$0")/.." && pwd)
prefix=${PREFIX:?}
if [ -f "$pkg/fail_install" ]; then
    cp "$pkg/bot/config.yml" "$prefix/bot/config.yml"
    install -m 0644 "$pkg/RELEASE" "$prefix/RELEASE"
    echo "version new" > "$prefix/VERSION"
    echo "install: boom" >&2
    exit 1
fi
install -m 0755 "$pkg/bin/uci" "$prefix/target/release/uci"
install -m 0644 "$pkg/bot/config.yml" "$prefix/bot/config.yml"
install -m 0644 "$pkg/RELEASE" "$prefix/RELEASE"
printf 'unit new\n' > "${SYMBIA_UNIT_DIR:?}/symbia-bot.service"
echo "installed"
EOF
    chmod +x "$d/deploy/install.sh"
    (cd "$d" && find . -type f ! -name SHA256SUMS ! -name RELEASE | sort | xargs sha256sum > SHA256SUMS)
    REL=$(sha256sum "$d/SHA256SUMS" | cut -c1-12)
    echo "$REL" > "$d/RELEASE"
    if [ "$variant" = tamper ]; then printf 'uci tampered\n' > "$d/bin/uci"; fi
    PKG=$W/pkgs/$n.tar.gz
    tar --sort=name --owner=0 --group=0 --numeric-owner -C "$W/pkgs/$n" -czf "$PKG" symbia-chess
    SHA=$(sha256sum "$PKG" | cut -c1-64)
}

# An archive with a hostile member. mkevil NAME KIND (dotdot, outside, symlink); sets PKG and SHA.
mkevil() {
    PKG=$W/pkgs/$1.tar.gz
    python3 - "$PKG" "$2" <<'PY'
import io, sys, tarfile
out, kind = sys.argv[1], sys.argv[2]
with tarfile.open(out, "w:gz") as t:
    def add(name, data=b"x"):
        ti = tarfile.TarInfo(name)
        ti.size = len(data)
        t.addfile(ti, io.BytesIO(data))
    add("symbia-chess/RELEASE", b"aaaaaaaaaaaa\n")
    if kind == "dotdot":
        add("symbia-chess/../evil")
    elif kind == "outside":
        add("other/evil")
    elif kind == "symlink":
        ti = tarfile.TarInfo("symbia-chess/link")
        ti.type = tarfile.SYMTYPE
        ti.linkname = "/etc/passwd"
        t.addfile(ti)
PY
    SHA=$(sha256sum "$PKG" | cut -c1-64)
}

# Runs apply.sh with the stand-ins first on PATH. Sets rc; the output is in $W/out.
run() {
    PATH=$root/shim:$PATH sh "$apply" "$@" > "$W/out" 2>&1
    rc=$?
}
run_stdin() {  # first argument: what to put on standard input
    in=$1
    shift
    printf '%s\n' "$in" | PATH=$root/shim:$PATH sh "$apply" "$@" > "$W/out" 2>&1
    rc=$?
}

no_leak() {  # nothing secret in anything apply wrote
    for f in "$W/out" "$SYMBIA_STATE/apply.log" "$SYMBIA_STATE/status" "$W/curl_argv.log" "$W/sys.log" "$W/journal.log"; do
        [ -f "$f" ] || continue
        if grep -q -e "$TOKEN" -e "$SIG" "$f"; then echo "secret in $f" >&2; return 1; fi
    done
    return 0
}

no_service_commands() {  # the service was neither stopped, killed, started nor restarted
    [ ! -f "$W/sys.log" ] || ! grep -q -e '^stop' -e '^kill' -e '^start' -e '^restart' "$W/sys.log"
}

line_of() { grep -n -e "$1" "$W/sys.log" | head -n 1 | cut -d: -f1; }

printf 'shell: %s\n\n' "$(readlink -f "$(command -v sh)")"

# ---- 1. nothing waiting -------------------------------------------------------------------------------------------
new_case noop
run run
t "run with nothing waiting exits 0" eq "$rc" 0
t "run with nothing waiting says nothing" test ! -s "$W/out"
t "run with nothing waiting touches no service" no_service_commands
t "run with no arguments prints usage and exits 1" sh -c "PATH=$root/shim:\$PATH sh '$apply' > /dev/null 2>&1; [ \$? = 1 ]"

# ---- 2. bad input changes nothing ----------------------------------------------------------------------------------
new_case bad_sha
mkpkg p ok
run stage "$PKG" "0000000000000000000000000000000000000000000000000000000000000000"
t "wrong checksum is refused" eq "$rc" 1
t "wrong checksum names the cause" has "$W/out" "does not match the checksum"
t "wrong checksum leaves nothing pending" absent "$SYMBIA_STATE/pending"
t "wrong checksum leaves the install untouched" eq "$(treehash)" "$before"
run stage "$PKG" "not-hex"
t "malformed checksum is refused" eq "$rc" 1

new_case tamper
mkpkg p tamper
run stage "$PKG" "$SHA"
t "a file changed inside the package is refused" eq "$rc" 1
t "tamper names the cause" has "$W/out" "own SHA256SUMS"
t "tamper leaves nothing pending or staged" sh -c "[ ! -e '$SYMBIA_STATE/pending' ] && [ ! -e '$SYMBIA_STATE/staging' ]"
t "tamper leaves the install untouched" eq "$(treehash)" "$before"

new_case dotdot
mkevil e dotdot
run stage "$PKG" "$SHA"
t "an archive with a ../ member is refused" eq "$rc" 1
t "dotdot names the cause" has "$W/out" "with .. in it"
t "dotdot wrote nothing outside" absent "$W/state/evil"
t "dotdot leaves nothing pending" absent "$SYMBIA_STATE/pending"
t "dotdot leaves the install untouched" eq "$(treehash)" "$before"

new_case outside
mkevil e outside
run stage "$PKG" "$SHA"
t "an archive with a member outside symbia-chess/ is refused" eq "$rc" 1
t "outside names the cause" has "$W/out" "outside symbia-chess"

new_case symlink
mkevil e symlink
run stage "$PKG" "$SHA"
t "an archive with a symlink is refused" eq "$rc" 1
t "symlink names the cause" has "$W/out" "links or special files"

# ---- 3. already installed -------------------------------------------------------------------------------------------
new_case same
mkpkg p ok
printf '%s\n' "$REL" > "$P/RELEASE"
before=$(treehash)
run stage "$PKG" "$SHA"
t "the release that is already installed is a no-op" eq "$rc" 0
t "already-installed says so" has "$W/out" "already installed"
t "already-installed touches no service" no_service_commands
t "already-installed leaves the install untouched" eq "$(treehash)" "$before"

# ---- 4. a busy bot is never touched ----------------------------------------------------------------------------------
new_case busy
mkpkg p ok
printf '{"nowPlaying":[{"gameId":"abcd1234"}]}\n' > "$W/playing.json"
run stage "$PKG" "$SHA"
t "busy: exit 4 (waiting)" eq "$rc" 4
t "busy: the package is staged and waiting" exists "$SYMBIA_STATE/pending/release"
t "busy: no stop, kill, start or restart" no_service_commands
t "busy: install untouched" eq "$(treehash)" "$before"
t "busy: the token reached Lichess through the config on stdin" has "$W/curl_stdin" "Bearer $TOKEN"
t "busy: the token is not in any command line" lacks "$W/curl_argv.log" "$TOKEN"
run run
t "busy: the timer's run also waits" eq "$rc" 4
t "busy: the waiting note is written once" eq "$(grep -c 'is waiting' "$SYMBIA_STATE/apply.log")" 1
t "busy: no stop, kill, start or restart after the second run either" no_service_commands
t "busy: no secrets in the output" no_leak

new_case unknown
mkpkg p ok
: > "$W/lichess_down"
run stage "$PKG" "$SHA"
t "cannot reach Lichess: exit 4 (counts as busy)" eq "$rc" 4
t "cannot reach Lichess: says unknown" has "$W/out" "bot is unknown"
t "cannot reach Lichess: no stop, kill, start or restart" no_service_commands
t "cannot reach Lichess: install untouched" eq "$(treehash)" "$before"

new_case notoken
mkpkg p ok
printf 'LICHESS_BOT_TOKEN=\n' > "$W/bot.env"
run stage "$PKG" "$SHA"
t "running service with no token on file counts as busy" eq "$rc" 4
t "no token: no stop, kill, start or restart" no_service_commands

new_case wait_loop
mkpkg p ok
printf '{"nowPlaying":[{"gameId":"abcd1234"}]}\n' > "$W/playing.json"
printf '2\n' > "$W/idle_after"
run stage "$PKG" "$SHA" --wait 20
t "--wait keeps checking until the game ends, then applies" eq "$rc" 0
t "--wait: three questions were asked" eq "$(cat "$W/curl_calls")" 3
t "--wait: the new config is installed" has "$P/bot/config.yml" "config new"

# ---- 5. the normal update -------------------------------------------------------------------------------------------
new_case idle_ok
mkpkg p ok
run stage "$PKG" "$SHA"
t "idle: applied, exit 0" eq "$rc" 0
t "idle: new config installed" has "$P/bot/config.yml" "config new"
t "idle: new engine installed" has "$P/target/release/uci" "uci new"
t "idle: RELEASE updated" eq "$(cat "$P/RELEASE")" "$REL"
t "idle: the service was stopped before it was started" sh -c "[ \"$(line_of '^stop')\" -lt \"$(line_of '^start')\" ]"
t "idle: game records untouched" has "$P/bot/games/game1.pgn" "game record"
t "idle: a snapshot of the old files was kept" exists "$SYMBIA_STATE/previous/files.tar"
t "idle: nothing left pending" absent "$SYMBIA_STATE/pending"
t "idle: status says ok" has "$SYMBIA_STATE/status" "last_result=ok"
t "idle: the log says the bot is connected" has "$SYMBIA_STATE/apply.log" "connected to Lichess"
t "idle: the service is running" eq "$(cat "$W/svc")" active
t "idle: no secrets in the output" no_leak
run run
t "idle: a second run has nothing to do" eq "$rc" 0
run status
t "status prints the installed release" has "$W/out" "installed release: $REL"
t "status shows no secrets" no_leak

# ---- 6. rollback ----------------------------------------------------------------------------------------------------
new_case health_fail
mkpkg p bad
printf 'old\n' > "$W/welcome_if"
run stage "$PKG" "$SHA"
t "health check fails: exit 1" eq "$rc" 1
t "health check fails: the install is exactly as before" eq "$(treehash)" "$before"
t "health check fails: the release is marked rejected" exists "$SYMBIA_STATE/rejected/$REL"
t "health check fails: nothing pending" absent "$SYMBIA_STATE/pending"
t "health check fails: status says rolled-back" has "$SYMBIA_STATE/status" "last_result=rolled-back"
t "health check fails: the old release came back" has "$SYMBIA_STATE/apply.log" "previous release is running again"
t "health check fails: stop, start, stop, start" eq "$(grep -c -e '^stop' -e '^start' "$W/sys.log")" 4
t "health check fails: game records untouched" has "$P/bot/games/game1.pgn" "game record"
run stage "$PKG" "$SHA"
t "a rejected release is refused the second time" eq "$rc" 1
t "rejected: names the cause" has "$W/out" "rejected earlier"
run run
t "rejected: the timer does nothing" eq "$rc" 0
run stage "$PKG" "$SHA" --force
t "--force tries a rejected release again" eq "$rc" 1
t "--force: and it rolls back again, exactly" eq "$(treehash)" "$before"
t "health check fails: no secrets in the output" no_leak

new_case install_fail
mkpkg p failinstall
run stage "$PKG" "$SHA"
t "installer fails halfway: exit 1" eq "$rc" 1
t "installer fails halfway: the install is exactly as before" eq "$(treehash)" "$before"
t "installer fails halfway: rejected" exists "$SYMBIA_STATE/rejected/$REL"
t "installer fails halfway: the old release was started again" eq "$(cat "$W/svc")" active
t "installer fails halfway: the failure is in the log" has "$SYMBIA_STATE/apply.log" "boom"

# A machine installed before the updater existed has no RELEASE and no VERSION. A failed first update must not leave
# the failed package's copies behind: status would then name a release that is not installed.
new_case install_fail_first
rm -f "$P/RELEASE" "$P/VERSION"
before=$(treehash)
mkpkg p failinstall
run stage "$PKG" "$SHA"
t "first update fails: exit 1" eq "$rc" 1
t "first update fails: the install is exactly as before" eq "$(treehash)" "$before"
t "first update fails: no RELEASE left behind" absent "$P/RELEASE"
t "first update fails: no VERSION left behind" absent "$P/VERSION"

new_case start_fails
mkpkg p ok
: > "$W/start_fails"
run stage "$PKG" "$SHA"
t "the service will not start: exit 1" eq "$rc" 1
t "the service will not start: the install is exactly as before" eq "$(treehash)" "$before"
t "the service will not start: says the old release did not come back" has "$W/out" "did not come back either"

# ---- 7. other service states ---------------------------------------------------------------------------------------
new_case stopped_service
mkpkg p ok
printf 'inactive\n' > "$W/svc"
run stage "$PKG" "$SHA"
t "stopped service: applied" eq "$rc" 0
t "stopped service: new config installed" has "$P/bot/config.yml" "config new"
t "stopped service: not started" no_service_commands
t "stopped service: Lichess was not asked" absent "$W/curl_calls"
t "stopped service: the log says it was left stopped" has "$W/out" "left stopped"

new_case stop_hangs
mkpkg p ok
: > "$W/stop_hangs"
run stage "$PKG" "$SHA"
t "a stop that hangs is killed after the limit, then the update goes on" eq "$rc" 0
t "stop hangs: the log says so" has "$W/out" "did not stop within"
t "stop hangs: kill was called" has "$W/sys.log" "^kill"
t "stop hangs: new config installed" has "$P/bot/config.yml" "config new"

new_case locked
mkpkg p ok
printf '{"nowPlaying":[{"gameId":"abcd1234"}]}\n' > "$W/playing.json"
run stage "$PKG" "$SHA"
t "locked: the package is waiting" eq "$rc" 4
printf '{"nowPlaying":[]}\n' > "$W/playing.json"
( flock 9; sleep 4 ) 9> "$SYMBIA_STATE/lock" &
sleep 1
run run
t "run while another apply holds the lock exits 0" eq "$rc" 0
t "run while another apply holds the lock says nothing" test ! -s "$W/out"
t "run while another apply holds the lock applies nothing" has "$P/bot/config.yml" "config old"
wait
run run
t "once the lock is free the timer's run applies the waiting package" eq "$rc" 0
t "once the lock is free the new config is installed" has "$P/bot/config.yml" "config new"

# ---- 8. fetch (signed link on standard input) ---------------------------------------------------------------------
new_case fetch_file
mkpkg p ok
run_stdin "file://$PKG" fetch "$SHA"
t "fetch from a file URL applies" eq "$rc" 0
t "fetch: new config installed" has "$P/bot/config.yml" "config new"
t "fetch: the downloaded file is removed" absent "$SYMBIA_STATE/incoming/package.tar.gz"

new_case fetch_http
mkpkg p ok
port=$((20000 + $$ % 20000))
(cd "$W/pkgs" && python3 -m http.server "$port" --bind 127.0.0.1 > /dev/null 2>&1) &
http_pid=$!
sleep 1
run_stdin "http://127.0.0.1:$port/p.tar.gz?X-Amz-Signature=$SIG" fetch "$SHA"
t "fetch from a signed-style link applies" eq "$rc" 0
t "fetch: the link's query string is nowhere in the output" no_leak
t "fetch: the link is not in any command line" lacks "$W/curl_argv.log" "$SIG"
new_case fetch_wrong
mkpkg p ok
run_stdin "http://127.0.0.1:$port/../pkgs/../nothing?X-Amz-Signature=$SIG" fetch "$SHA"
t "fetch of a missing file fails" eq "$rc" 1
t "fetch failure: names the download, not the link" has "$W/out" "the download failed"
t "fetch failure: the link is nowhere in the output" no_leak
t "fetch failure: install untouched" eq "$(treehash)" "$before"
new_case fetch_nolink
run_stdin "" fetch "0000000000000000000000000000000000000000000000000000000000000000"
t "fetch with no link on standard input fails" eq "$rc" 1
kill "$http_pid" 2> /dev/null
http_pid=

# ---- 9. the payload push.sh sends ---------------------------------------------------------------------------------
new_case direct
mkpkg p ok
run stage "$PKG" "$SHA"
direct=$(treehash)
new_case via_push
mkpkg p ok
export SYMBIA_NO_SYSTEMD=1
sh "$push" --print-payload "file://$PKG" "$SHA" 30 > "$W/payload.sh"
t "the payload has the updater inside it" has "$W/payload.sh" "symbia-apply: put a new package on this machine"
t "the payload ends the embedded script exactly once" eq "$(grep -c '^SYMBIA_APPLY_END$' "$W/payload.sh")" 1
PATH=$root/shim:$PATH sh < "$W/payload.sh" > "$W/out" 2>&1
rc=$?
unset SYMBIA_NO_SYSTEMD
t "the payload runs under sh and applies the package" eq "$rc" 0
t "the payload installed the updater" exists "$W/bin/symbia-apply"
t "the installed updater is the one in the repository" cmp "$apply" "$W/bin/symbia-apply"
t "the payload prints the status at the end" has "$W/out" "installed release: $REL"
t "the result matches a direct run" eq "$(treehash)" "$direct"
sh "$push" --print-payload "file://x'y" "$SHA" 30 > /dev/null 2>&1
t "a link with a quote in it is refused" ne "$?" 0
sh "$push" --print-payload "file://x" "short" 30 > /dev/null 2>&1
t "a short checksum is refused" ne "$?" 0
sh "$push" nothing nothing nothing nothing "file://x" "$SHA" > /dev/null 2>&1
t "push with missing key files is refused" ne "$?" 0

# ---- 10. setup ------------------------------------------------------------------------------------------------------
new_case setup
run setup
t "setup exits 0" eq "$rc" 0
t "setup installs the updater" exists "$W/bin/symbia-apply"
t "setup writes the service unit with the right path" has "$W/units/symbia-apply.service" "ExecStart=$W/bin/symbia-apply run"
t "setup writes the timer" has "$W/units/symbia-apply.timer" "OnUnitActiveSec=60s"
t "setup enables the timer" has "$W/sys.log" "enable --now symbia-apply.timer"
run setup
t "setup is repeatable" eq "$rc" 0
if command -v systemd-analyze > /dev/null 2>&1; then
    t "systemd-analyze verify accepts the units" sh -c "cd '$W/units' && systemd-analyze verify ./symbia-apply.service ./symbia-apply.timer 2>&1 | grep -v -e 'Failed to create' -e 'symbia-bot' -e '^$' | grep -q -e 'Unknown' -e 'Invalid' -e 'Failed to parse' && exit 1; exit 0"
fi

printf '\n%s passed, %s failed\n' "$pass" "$fail"
[ "$fail" = 0 ]
