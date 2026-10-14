// Ids for the messages this page sends. The agent keeps a message's id as
// the id of the turn it starts and records it in the conversation, so an id
// has to stay unique across page loads, tabs and devices, not just within
// one page.

const HEX = "0123456789abcdef";

/** A version 4 UUID from `bytes`, which must hold 16 random bytes. */
function formatUuid(bytes: Uint8Array): string {
  // Version 4, RFC 4122 variant.
  bytes[6] = ((bytes[6] ?? 0) & 0x0f) | 0x40;
  bytes[8] = ((bytes[8] ?? 0) & 0x3f) | 0x80;
  let out = "";
  bytes.forEach((byte, index) => {
    if (index === 4 || index === 6 || index === 8 || index === 10) out += "-";
    out += HEX.charAt(byte >> 4) + HEX.charAt(byte & 0x0f);
  });
  return out;
}

/**
 * A random UUID. `crypto.randomUUID` exists only in secure contexts (HTTPS
 * and localhost), and the app is also served over plain HTTP on a LAN, where
 * `crypto.getRandomValues` is still available. Without any `crypto`, the id
 * is made from the clock and `Math.random`, which still keeps two page loads
 * apart.
 */
function randomUuid(): string {
  const source = (globalThis as { crypto?: Partial<Crypto> }).crypto;
  if (typeof source?.randomUUID === "function") return source.randomUUID();
  const bytes = new Uint8Array(16);
  if (typeof source?.getRandomValues === "function") {
    source.getRandomValues(bytes);
  } else {
    let seed = Date.now();
    for (let i = 0; i < bytes.length; i++) {
      bytes[i] = Math.floor(Math.random() * 256) ^ (seed & 0xff);
      seed = Math.floor(seed / 256) || Date.now();
    }
  }
  return formatUuid(bytes);
}

/** A new id for a message this page is about to send. */
export function newMessageId(): string {
  return `web-${randomUuid()}`;
}
