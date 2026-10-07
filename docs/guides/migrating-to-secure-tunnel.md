# Migrating to the secure tunnel

When Residuum Cloud's relay supports it, Residuum connects over the secure tunnel: it gets its own certificate for your addresses and terminates TLS itself, so the relay only passes encrypted bytes along. This guide covers what changes for you. How it works is in [Secure Tunnel and Certificates](../systems-usage/secure-tunnel.md).

## What happens when you update

- Residuum tries the secure tunnel first. A relay that doesn't offer it yet is served over the older tunnel, exactly as before.
- On the secure tunnel the first connection sets up the install: it creates a certificate account, shows a **recovery code**, waits a few minutes for a DNS record, and gets a certificate. Until the certificate is installed, your Residuum addresses don't load. Settings, All agents, Residuum Cloud, Remote access (or `residuum remote status`) shows the progress.
- Save the recovery code when it is shown, then confirm it. It is the only way to take your address back if every install's certificate account is lost.

## If you run more than one instance

An address belongs to the first instance of your account that sets itself up. Every other instance shows "needs another instance" and serves nothing through Residuum Cloud until it joins that one; local access keeps working. To make a different instance the owner instead, use its recovery code: Remote access, Use a recovery code, or `residuum remote reset-pins`.

## What stays the same

- Opening Residuum on the machine it runs on.
- Paired browsers keep working, once the new certificate is installed. The cookie names and pairing links come from the identity the install stored when it set up.
- Staying on the older tunnel: set `remote_access = false` under `[cloud]` in `hub/config.toml`.

## One-way switch

An install that has completed the secure-tunnel setup never falls back to the older tunnel, even when the relay answers as if it didn't support the secure one. The older tunnel lets the relay read everything, so a relay that could force the fallback could read your traffic. If the relay stops supporting the secure tunnel, remote access stays down and the status says why; set `remote_access = false` to accept the older tunnel on purpose.
