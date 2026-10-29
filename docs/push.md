# Pushing a release to the hosted bot

How B15 went out on 23 October (release 91e671eec2c9), written down so the next push does not depend on anyone's memory. Nothing here needs the bot's token. The Lightsail instance is symbia-bot-2 in us-east-1; its only inbound rule is port 22 with the Lightsail console alias and no address, which is the state to restore at the end.

## What a push needs

* A package from `tools/package.sh path/to/uci` run on Linux (GNU tar and sha256sum), with a musl release binary built from the same source as the Mac tree. Before packaging, compare the SHA-256 of the files the package copies (`bot/`, `tools/hook_smoke.py`, `tools/fingerprint.py`, `deploy/`) between the Mac and the build tree.
* AWS access that can call Lightsail and S3 (the connector the cloud session uses). The Mac has no AWS credentials.
* The person's yes to opening port 22 to the Mac's address for about a minute.

## Steps

1. **Read the current rule** with `GetInstancePortStates`. `PutInstancePublicPorts` replaces every rule, so what it sends must include whatever is there now. Read the Mac's public address (`curl https://checkip.amazonaws.com` on the Mac).
2. **Upload the package.** Presign a PUT for `tmp/push-<name>/package.tar.gz` in the staging bucket and send the file with `curl -T`.
3. **Presign four GET links** (the package, `key`, `cert`, `known_hosts`), lifetime 900 seconds, the longest S3 allows. Write them into an env file on the Mac (`push-<name>.env`, single-quoted) and write the Mac script below beside it.
4. **In one AWS script:** open port 22 to `<mac address>/32` plus the `lightsail-connect` alias; call `GetInstanceAccessDetails` for ssh; upload the private key, then the host keys as `known_hosts` lines (`<ip> <algorithm> <publicKey>`), then the certificate last, each with `PutObject` from inside the script so the key never appears in the conversation. The certificate lives about 13 minutes, so the Mac script runs next.
5. **Run the Mac script** with `exec` (timeout 900000 ms). It fetches the three files, runs `deploy/push.sh` with `--wait 120`, and removes what it fetched when it ends. `push.sh` sends the updater and the package link over one ssh session. Exit 0 means applied; exit 4 means the package is on the machine and waiting for the game to end, and the box's timer applies it within a minute of the game ending.
6. **Close up, in this order:** restore the port rule (no address, alias kept); delete the four objects and list the prefix to see it is empty; look for leftovers on the Mac (the env file and the temp folder).
7. **Record** a ledger observation: the release name, when the port opened and closed, what the apply log said, and what to check in the next game.

## The Mac script

```sh
#!/bin/sh
set -eu
here=$HOME/Repos/chess-stage-<name>
. "$here/push-<name>.env"          # PKG_SHA, PKG_URL, KEY_URL, CERT_URL, HOSTS_URL
tmp=$(mktemp -d "${TMPDIR:-/tmp}/symbia-push.XXXXXX")
cleanup() {
    rm -f "$tmp/key" "$tmp/key-cert.pub" "$tmp/known_hosts" "$here/push-<name>.env"
    rmdir "$tmp" 2>/dev/null || true
}
trap cleanup EXIT INT TERM
fetch() { curl -fsS --max-time 60 -o "$2" "$1"; }
fetch "$KEY_URL" "$tmp/key"
fetch "$CERT_URL" "$tmp/key-cert.pub"
fetch "$HOSTS_URL" "$tmp/known_hosts"
chmod 600 "$tmp/key"
cd "$HOME/chess"
rc=0
sh deploy/push.sh <ip> "$tmp/key" "$tmp/key-cert.pub" "$tmp/known_hosts" "$PKG_URL" "$PKG_SHA" 120 || rc=$?
echo "PUSH_EXIT=$rc"
exit "$rc"
```

The certificate must be named after the key with `-cert.pub` on the end; ssh pairs them by name.

## Things that went wrong before, and what stops them now

* A push job killed by the 120 second default left its key on the Mac (B14). The job gets a long timeout and the script removes its own files on exit; step 6 checks anyway.
* The exec tool refuses command text containing `rm -rf`; the script removes files one by one and `rmdir`s the folder.
* Signed links stop working after 900 seconds, so presign after the package is uploaded, not before.

## Still not built

Pull-based updates, signed packages and a standing health line on the box. Each push still needs a session with AWS access and a yes for the firewall step.
