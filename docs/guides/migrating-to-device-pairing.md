# Migrating to device pairing

Residuum reached through Residuum Cloud now answers only browsers you have paired with it. Signing in on the relay no longer opens your install by itself. Nothing changes when you open Residuum on the machine it runs on. This guide covers what to do before and after updating, and what behaves differently.

## Before you update

**If you only ever reach Residuum remotely**, such as an install on a server, make sure you can open a shell on the machine (SSH) before you update. Updating from the web app through Residuum Cloud restarts Residuum into the version that asks for pairing, and the browser you updated from is then refused. The first browser can only be paired from the machine Residuum runs on:

1. Update. From the shell, `residuum update` does it.
2. Once Residuum has restarted and reconnected to Residuum Cloud, run `residuum remote pair` on the machine.
3. Open the printed link in the browser you use, or scan the QR code with your phone.
4. Save the ten recovery codes it prints.

If you already updated and can't reach Residuum, the same `residuum remote pair` over SSH fixes it. It works as long as the daemon is running and Residuum Cloud has connected once since the update.

**If you use Residuum on the machine itself**, there is nothing to do first. Pair your other browsers afterwards (see [Reach Residuum from your phone and other browsers](remote-access.md)).

## What behaves differently

- **Every browser that reaches Residuum through Residuum Cloud has to pair once**, including ones you used before. Until then it lands on the pairing page, and its API calls answer `401`.
- **Workbench pages need pairing too**, separately for the workbench address. The Open button in Residuum does this for you. A saved link to an artifact sends an unpaired browser to the artifact inside the Residuum app.
- **Writes through Residuum Cloud are checked for where they came from.** A request that changes something, or opens a socket, must say it came from the same origin. Browsers do this on their own. A script or bookmarklet on another site can no longer act with your cookie.
- **Making a pairing link is local-only.** A paired browser can approve other browsers, but can't make first-device links.
- **A2A and Microsoft Teams are unaffected.** They authenticate their own callers.
- **Several instances of one account** each keep their own pairing, because the cookie carries the instance's name. Pair each instance once per browser.
- **Pages on the workbench address can't manage pairing.** An artifact page gets `403` from every route that approves browsers or makes pairing links.

## After you update

1. Open Settings, All agents, Residuum Cloud on the machine, and check Paired browsers lists the browsers you expect.
2. Keep the recovery codes somewhere you can find them. Make new ones from the same place if you lose them.
