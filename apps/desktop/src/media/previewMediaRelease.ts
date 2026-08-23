type ReleasableMediaElement = Pick<
  HTMLMediaElement,
  "load" | "pause" | "removeAttribute"
>;

/** Stop playback and make WebKit tear down the element's decoder. */
export function releasePreviewMediaElement(element: ReleasableMediaElement): void {
  element.pause();
  element.removeAttribute("src");
  element.load();
}
