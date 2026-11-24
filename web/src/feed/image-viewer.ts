export interface ViewerImage {
  src: string;
  alt: string;
}

export function viewImageLabel(alt: string): string {
  return `View full size: ${alt}`;
}
