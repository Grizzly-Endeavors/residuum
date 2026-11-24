// Files dragged over the page. A browser opens a file dropped on a page that
// doesn't take it, which leaves the app; the app refuses those drops, and a
// place that attaches files takes the ones that land on it.

export function carriesFiles(event: DragEvent): boolean {
  return event.dataTransfer?.types.includes("Files") ?? false;
}

export function refuseStrayFileDrops(event: DragEvent): void {
  if (event.defaultPrevented || !carriesFiles(event)) return;
  event.preventDefault();
  if (event.type === "dragover" && event.dataTransfer) event.dataTransfer.dropEffect = "none";
}
