# Reach Residuum from your phone and other browsers

With Residuum Cloud connected, you can use Residuum from any browser at your personal address. Residuum answers only browsers you have paired with it, so signing in to the cloud account isn't enough on its own. This guide pairs your first browser, adds more, and removes one you no longer trust. How it works is in [Remote access](../systems-usage/remote-access.md).

You need Residuum Cloud connected first (Settings, All agents, Residuum Cloud). Opening Residuum on the machine it runs on never asks for pairing.

## Check that the secure tunnel is ready

Residuum connects over the secure tunnel and gets its own certificate for your addresses. Open Settings, All agents, Residuum Cloud, Remote access, or run `residuum remote status`. It says Ready once the certificate is installed. The first time takes a few minutes, because Residuum waits for a DNS record to appear.

1. **Save the recovery code.** The first time, a recovery code is shown on the machine Residuum runs on. It is the only way to take your address back if every instance's certificate account is lost. Save it, then choose I've saved it (or run `residuum remote saved`). Residuum stops keeping it.
2. **A second instance says it needs another instance.** An address belongs to the first instance that set it up. A later instance serves nothing remotely until it joins that one. To take the address over instead, choose Use a recovery code (or run `residuum remote reset-pins`).
   If you lost the recovery code, choose Lost your recovery code? Email me a reset link in the same dialog (or run `residuum remote email-reset`). Residuum Cloud's pin service emails the address on your account; open the link and confirm. After a waiting period (24 hours), this instance's certificate account becomes the only one allowed and the new recovery code shows in Remote access. Until then nothing changes, and `residuum remote status` says when it completes. If you didn't ask for it, use the cancel link in the email.
3. **A warning about an unrecognized certificate account** means an account this instance never approved can get certificates for your addresses. If you didn't add it, treat it as a sign someone else controls your relay account.
4. **A warning about another instance's reset.** If another instance asks for a reset by email while this one is set up, Remote access says so, and you can choose Cancel the reset (or run `residuum remote cancel-reset`). Cancelling keeps your address where it is.

## Pair your first browser

Do this on the machine Residuum runs on.

1. Open Settings, All agents, Residuum Cloud. Under Paired browsers, choose Enable remote access.
2. Save the ten recovery codes it shows. They appear only this once, and each pairs one browser if you ever lose access to every paired one.
3. Open the link in the browser you want to use through Residuum Cloud, or scan the QR code with your phone. The link works once and expires after 10 minutes.
4. On the page that opens, name the browser and choose Pair this browser. Residuum opens.

**On a server with no screen**, run `residuum remote pair` over SSH instead. It prints the same link and a QR code in the terminal.

## Pair another browser

1. On the new browser, open your Residuum Cloud address. It lands on the pairing page.
2. Name the browser and choose Ask for approval. The page shows a six-character code.
3. On a browser that is already paired, or in Settings on the machine Residuum runs on, open Residuum Cloud, Paired browsers. The request appears with its code and name.
4. Check that the code matches the one on the new browser's screen, then choose Approve. The new browser opens Residuum within a couple of seconds. Choose Refuse if you don't recognize the request.

A request waits up to 10 minutes, and at most 10 can wait at once.

## Use a recovery code

If no paired browser is within reach, open the pairing page, choose Use a recovery code instead, and enter one of the codes. Each code works once. In Settings, Paired browsers, Make new recovery codes replaces all ten.

## Open workbench pages

Workbench pages open on their own address, which holds its own pairing. Choosing Open on an artifact in the Residuum app takes care of it: the new tab hands your browser's pairing over and then shows the page. A link to an artifact opened in a browser that hasn't done that yet takes you to the same artifact in the Residuum app.

## Remove a browser

In Settings, Residuum Cloud, Paired browsers, choose Revoke on the browser's row, or run `residuum remote devices` and `residuum remote revoke <id>`. The browser is refused on its next request, on both the app and the workbench. Pair it again to let it back in.

## If something doesn't work

- **Making the link says Residuum Cloud hasn't announced an address.** Residuum learns its address when it connects to the relay. Wait for Residuum Cloud to show Connected, then try again.
- **A pairing link says it expired or was used.** Links work once for 10 minutes. Make a new one.
- **Pairing says there were too many attempts.** Each address can try 10 times a minute. Wait a minute.
- **A browser keeps landing on the pairing page.** Its cookie was removed or the device was revoked, or Residuum's address for this instance changed. Pair it again.
