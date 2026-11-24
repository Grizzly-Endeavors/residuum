import { toast } from "./toast.svelte";

export function notifyStagedRemoval(message: string, putBack: () => void): void {
  toast.success(message, { label: "Undo", onClick: putBack });
}
