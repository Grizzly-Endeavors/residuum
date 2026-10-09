// What the image viewer shows, and the words a picture's button uses for it.

/** One picture for the viewer: where it is, and the words that stand for it. */
export interface ViewerImage {
  src: string;
  /** What the picture shows, in words: the viewer's title and the picture's alt text. */
  alt: string;
}

/** What a picture's button is called: "View full size: Attached image 2". */
export function viewImageLabel(alt: string): string {
  return `View full size: ${alt}`;
}
