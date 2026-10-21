#!/bin/bash
# Lightsail launch script: runs once, as root, the first time the instance boots.
#
# This file is the template. The @...@ values are filled in when the instance is created: four short-lived signed S3
# links (the package to install, where this script's log goes, and where the bot token will be left and then blanked)
# and the time after which the links stop working. The links last 15 minutes from the moment they are signed, so
# everything here has to be done by then.
#
# The log is the only way to see what happened on a machine nobody has logged in to, so it is uploaded after each stage
# and again when the script ends, however it ends. Anything shaped like a Lichess token is blanked out of it first.
#
# The token is never in this file. Brian's terminal sends it to the S3 link; this script waits for it, asks Lichess
# whether it belongs to the Symbia bot account, writes it to the one root-only file the service reads, starts the
# service and overwrites the S3 copy.
exec > /var/log/symbia-firstboot.log 2>&1
chmod 600 /var/log/symbia-firstboot.log
set -eu

PKG_URL='@PKG_URL@'
REPORT_URL='@REPORT_URL@'
TOKEN_GET_URL='@TOKEN_GET_URL@'
TOKEN_PUT_URL='@TOKEN_PUT_URL@'
DEADLINE=@DEADLINE@

report() { sed 's/lip_[A-Za-z0-9_]*/lip_REDACTED/g' /var/log/symbia-firstboot.log | curl -fsS -m 60 -X PUT --data-binary @- "$REPORT_URL" || true; }
blank() { printf 'blank\n' | curl -fsS -m 30 -X PUT --data-binary @- "$TOKEN_PUT_URL" > /dev/null || true; }
trap report EXIT
step() { echo "== $(date -u +%T) $*"; }

export DEBIAN_FRONTEND=noninteractive
step "apt"
apt-get -o DPkg::Lock::Timeout=600 update -y
apt-get -o DPkg::Lock::Timeout=600 install -y git python3-venv curl

step "package"
mkdir -p /root/pkg
curl -fsSL -m 180 -o /root/pkg.tgz "$PKG_URL"
tar -xzf /root/pkg.tgz -C /root/pkg

step "install"
/root/pkg/symbia-chess/deploy/install.sh
echo "service enabled: $(systemctl is-enabled symbia-bot)  active: $(systemctl is-active symbia-bot || true)"
systemd-analyze verify /etc/systemd/system/symbia-bot.service && echo "unit verify: clean"

step "machine"
uname -srm
echo "cpus: $(nproc)"
free -m | sed -n 1,2p
grep -m1 'model name' /proc/cpuinfo || true

# The service has never been started on a real machine. Start it once with a token that cannot work, and read what it
# does: it should get past the engine check (that needs the user, the paths and the environment file to be right) and
# then be turned away by Lichess. That way a mistake in the unit shows here, not in front of the real token.
step "service dry run with a token that is not one"
printf 'LICHESS_BOT_TOKEN=not-a-token\n' > /etc/symbia/bot.env
systemctl start symbia-bot
sleep 12
journalctl -u symbia-bot -n 25 --no-pager | cut -c1-200 || true
systemctl stop symbia-bot || true
systemctl reset-failed symbia-bot || true
printf 'LICHESS_BOT_TOKEN=\n' > /etc/symbia/bot.env
chmod 600 /etc/symbia/bot.env
report

step "waiting for the token"
code=none
rm -f /root/token.in
while [ "$(date +%s)" -lt "$DEADLINE" ]; do
    code=$(curl -s -m 15 -o /root/token.in -w '%{http_code}' "$TOKEN_GET_URL" || true)
    if [ "$code" = "200" ] && [ -s /root/token.in ]; then break; fi
    sleep 4
done
if [ "$code" != "200" ]; then
    echo "no token arrived before the links expired (last answer: $code)"
    exit 1
fi

step "token received"
t=$(tr -d ' \r\n' < /root/token.in)
rm -f /root/token.in
if ! printf '%s' "$t" | grep -Eq '^[A-Za-z0-9_]{16,}$'; then
    echo "what arrived does not look like a Lichess token (length ${#t}); not using it"
    echo "characters outside letters, digits and underscore: $(printf '%s' "$t" | tr -d 'A-Za-z0-9_' | od -An -c | tr -s ' ')"
    blank
    exit 1
fi
who=$(curl -fsS -m 20 -A 'symbia-chess-firstboot' -H "Authorization: Bearer $t" https://lichess.org/api/account \
      | python3 -c 'import sys, json; d = json.load(sys.stdin); print(d.get("username"), d.get("title"))') || who=""
echo "Lichess answered for this token: ${who:-nothing usable}"
if [ "$who" != "Symbia BOT" ]; then
    echo "that is not the Symbia bot account; not using it"
    blank
    exit 1
fi
( umask 077; printf 'LICHESS_BOT_TOKEN=%s\n' "$t" > /etc/symbia/bot.env )
chmod 600 /etc/symbia/bot.env
blank
unset t

step "start"
systemctl start symbia-bot
sleep 20
echo "service enabled: $(systemctl is-enabled symbia-bot)  active: $(systemctl is-active symbia-bot || true)"
journalctl -u symbia-bot -n 30 --no-pager | cut -c1-200 || true
step "first boot finished"
report
