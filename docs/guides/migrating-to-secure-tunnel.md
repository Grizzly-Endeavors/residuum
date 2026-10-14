# Migrating to the secure tunnel

Residuum Cloud connects over the secure tunnel only. Residuum gets its own certificate for your addresses and terminates TLS itself, so the relay only passes encrypted bytes along. The older tunnel, which let the relay read everything, is gone from Residuum and from the relay. This guide covers what changes for an install that was on the older tunnel. How the secure tunnel works is in [Secure Tunnel and Certificates](../systems-usage/secure-tunnel.md).

## What happens when you update

- On the first start after the update, Residuum connects to the relay's secure tunnel. If this install never set it up, the first connection creates a certificate account, shows a **recovery code**, waits a few minutes for a DNS record, and gets a certificate. Until the certificate is installed, your Residuum addresses don't load. Settings, All agents, Residuum Cloud, Remote access (or `residuum remote status`) shows the progress.
- Save the recovery code when it is shown, then confirm it. It is the only way to take your address back if every install's certificate account is lost.
- Paired browsers keep working once the certificate is installed. The cookie names and pairing links come from the identity the install stored when it set up.

## If you run more than one instance

An address belongs to the first instance of your account that sets itself up. Every other instance shows "needs another instance" and serves nothing through Residuum Cloud until it joins that one; local access keeps working. To make a different instance the owner instead, use its recovery code: Remote access, Use a recovery code, or `residuum remote reset-pins`.

Your instances reach each other's agents only after joining: a sibling that never joined is not called, and the relay no longer vouches for a caller. Join each pair from Remote access (or `residuum remote join {slug}`) so the A2A calls between them work again.

## Configuration

- `remote_access = false` under `[cloud]` no longer keeps the older tunnel, because there isn't one. The key is still accepted so the config loads, and Residuum logs a warning that it has no effect. To stay off the cloud, set `enabled = false` under `[cloud]`, remove the section, or disconnect in Settings.
- `local_port` under `[cloud]` is still accepted and has no effect. Remove it.
- `relay_url` may still end in `/tunnel/register`; Residuum registers at `/tunnel/v2/register` on the same host. New configs, and the Relay URL field in Settings, use the `/tunnel/v2/register` form.

## What stays the same

- Opening Residuum on the machine it runs on.
- Which browsers may connect: only paired ones.
- Teams and A2A on your instance's own address (`https://{slug}.{user}.agent-residuum.com`).
