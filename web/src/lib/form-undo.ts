// Undo for removing an entry from a settings form list (providers, MCP
// servers, webhooks). The removal is a staged change in the settings model,
// so nothing is written until Save changes: Undo puts the entry back in the
// form, as Discard would with every other staged change.

import { toast } from "./toast.svelte";

/** Say an entry was removed from a form, with an Undo that puts it back. Call it right after the removal. */
export function notifyStagedRemoval(message: string, putBack: () => void): void {
  toast.success(message, { label: "Undo", onClick: putBack });
}
