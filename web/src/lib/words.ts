// How the UI puts things into words where several places say the same thing.

/** Names as a sentence lists them: "atlas", "atlas and scout", "atlas, scout and wren"; `word` joins the last two. */
export function joinNames(names: readonly string[], word = "and"): string {
  if (names.length <= 1) return names.join("");
  return `${names.slice(0, -1).join(", ")} ${word} ${names[names.length - 1] ?? ""}`;
}
