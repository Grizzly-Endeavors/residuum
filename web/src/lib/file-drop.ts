// Files dragged over the page. A browser opens a file dropped on a page that
// doesn't take it, which leaves the app; the app refuses those drops, and a
// place that attaches files takes the ones that land on it.

/** The drag is carrying files from outside the page, not text or an element from it. */
export function carriesFiles(event: DragEvent): boolean {
  return event.dataTransfer?.types.includes("Files") ?? false;
}

/**
 * A listener for the window's `dragover` and `drop`: a drop of files that no
 * place took (its handler would have called `preventDefault`) is stopped here,
 * so the browser doesn't navigate to the file, and the pointer shows that it
 * can't be dropped.
 */
export function refuseStrayFileDrops(event: DragEvent): void {
  if (event.defaultPrevented || !carriesFiles(event)) return;
  event.preventDefault();
  if (event.type === "dragover" && event.dataTransfer) event.dataTransfer.dropEffect = "none";
}
