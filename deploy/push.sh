#!/bin/sh
# Push a package to the hosted bot over SSH. This runs on the Mac (or any machine with an ssh client).
#
#   deploy/push.sh IP KEY KEY-cert.pub KNOWN_HOSTS PACKAGE_URL SHA256 [WAIT_SECONDS]
#       Logs in once with the temporary key and certificate Lightsail issued for the instance (the certificate file must
#       be named after the key with -cert.pub on the end, because that is how ssh pairs them), and in that one session:
#       installs this checkout's deploy/apply.sh as /usr/local/sbin/symbia-apply, runs `symbia-apply setup`, hands the
#       package link to `symbia-apply fetch` on standard input, and prints `symbia-apply status`.
#       WAIT_SECONDS (default 600) is how long symbia-apply keeps waiting for a game in progress to end.
#
#   deploy/push.sh --print-payload PACKAGE_URL SHA256 [WAIT_SECONDS]
#       Prints the script that would be sent and does nothing else. tests/apply_test.sh runs it under sh.
#
# The temporary certificate Lightsail issues is valid for about 13 minutes (measured), so this should be the next thing run.
# The package link is sent on the ssh session's standard input and is never on a command line, here or there.
# Each push carries its own copy of the updater, so a broken updater is repaired by the next push.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
user=${SYMBIA_SSH_USER:-ubuntu}

die() { printf 'push: %s\n' "$*" >&2; exit 1; }

payload() {  # link sha256 wait
    case $1 in *"'"*) die "the package link has a quote in it" ;; esac
    case $2 in *[!0-9a-f]*|'') die "the checksum must be 64 lowercase hex characters" ;; esac
    [ "${#2}" -eq 64 ] || die "the checksum must be 64 lowercase hex characters"
    case $3 in ''|*[!0-9]*) die "the wait must be a whole number of seconds" ;; esac
    if grep -q '^SYMBIA_APPLY_END$' "$here/apply.sh"; then die "apply.sh contains the end marker"; fi
    cat <<'HEAD'
set -eu
BIN=${SYMBIA_BIN:-/usr/local/sbin}
mkdir -p "$BIN"
cat > "$BIN/.symbia-apply.new" <<'SYMBIA_APPLY_END'
HEAD
    cat "$here/apply.sh"
    cat <<'MID'
SYMBIA_APPLY_END
chmod 755 "$BIN/.symbia-apply.new"
mv -f "$BIN/.symbia-apply.new" "$BIN/symbia-apply"
"$BIN/symbia-apply" setup
rc=0
MID
    printf "printf '%%s\\\\n' '%s' | \"\$BIN/symbia-apply\" fetch %s --wait %s || rc=\$?\n" "$1" "$2" "$3"
    cat <<'TAIL'
"$BIN/symbia-apply" status || true
exit "$rc"
TAIL
}

if [ "${1:-}" = "--print-payload" ]; then
    [ $# -ge 3 ] || die "usage: push.sh --print-payload PACKAGE_URL SHA256 [WAIT_SECONDS]"
    payload "$2" "$3" "${4:-600}"
    exit 0
fi

[ $# -ge 6 ] || die "usage: push.sh IP KEY KEY-cert.pub KNOWN_HOSTS PACKAGE_URL SHA256 [WAIT_SECONDS]"
ip=$1
key=$2
cert=$3
known=$4
link=$5
sha=$6
wait=${7:-600}
for f in "$key" "$cert" "$known"; do [ -f "$f" ] || die "no such file: $f"; done
[ "$cert" = "$key-cert.pub" ] || die "the certificate must be named $key-cert.pub, because ssh pairs a certificate with its key by name"
verbose=
[ -z "${SYMBIA_SSH_VERBOSE:-}" ] || verbose=-vv
payload "$link" "$sha" "$wait" > /dev/null  # checks the arguments here, where a failure stops the script

set +e
payload "$link" "$sha" "$wait" | ssh -T $verbose \
    -F /dev/null -i "$key" -o IdentitiesOnly=yes \
    -o "UserKnownHostsFile=$known" -o GlobalKnownHostsFile=/dev/null -o StrictHostKeyChecking=yes \
    -o BatchMode=yes -o ConnectTimeout=15 -o ServerAliveInterval=15 -o ServerAliveCountMax=40 \
    "$user@$ip" 'sudo sh -s'
rc=$?
set -e
case $rc in
    0) echo "push: applied" ;;
    4) echo "push: the package is on the machine and waiting for the bot to finish its game; the machine will apply it" ;;
    *) echo "push: failed with status $rc" >&2 ;;
esac
exit "$rc"
