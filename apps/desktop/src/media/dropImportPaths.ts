export type DroppedFileLike = {
  path?: string;
};

export function droppedImportPaths(files: Iterable<DroppedFileLike>): string[] {
  const paths: string[] = [];
  const seen = new Set<string>();
  for (const file of files) {
    const path = file.path?.trim();
    if (!path || seen.has(path)) continue;
    seen.add(path);
    paths.push(path);
  }
  return paths;
}
