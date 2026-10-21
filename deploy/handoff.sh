#!/bin/bash
# Sends the Lichess bot token for the Symbia account to the new Lightsail machine, once.
#
# This is a template. At launch the @PUT_URL@ value is replaced by a signed link that can only write one object in a
# private bucket and stops working about 15 minutes after it is made. The filled copy goes in dist/, which git ignores,
# and deletes itself after a successful send. The token is read without echo and goes straight from the terminal to
# that link: it is not written to a file, not put on a command line, and never shown to Claude.
URL='@PUT_URL@'

printf 'Lichess bot token for Symbia (nothing shows while you paste): '
IFS= read -rs t
echo
t=$(printf '%s' "$t" | tr -d ' \r\n')
if [ "${#t}" -lt 16 ]; then
    echo "That is too short to be a token. Nothing was sent." >&2
    exit 1
fi
if printf '%s' "$t" | curl -fsS -m 30 -X PUT --data-binary @- "$URL"; then
    echo "Sent. Symbia should be online shortly. You can close this window."
    rm -f -- "$0"
else
    echo "The send failed (the link may have expired). Nothing was kept. Tell Claude." >&2
    exit 1
fi
