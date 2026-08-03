export type TimelineCommit = {
  commitHash: string;
  timelineHash?: string;
  header?: string;
  fullMessage?: string;
  parents: string[];
};

type LogicalHistory = {
  currentRef: string | null;
  undoRefs: string[];
};

const RESTORED_REF_MARKER = "Montage-Restored-Ref:";
const RESTORED_PARENT_MARKER = "Montage-Restored-Parent:";
const RESTORE_HEADER_PREFIX = "Restore timeline to ";
const RESTORE_REASON =
  "Restored project.otio.json from the desktop timeline history panel.";

/** Build logical edit history while treating restore audit commits as pointers. */
export function logicalHistory(commits: TimelineCommit[]): LogicalHistory {
  const head = commits[0];
  if (!head) return { currentRef: null, undoRefs: [] };
  const byHash = new Map(commits.map((commit) => [commit.commitHash, commit]));
  const restoredParents = new Map<string, string>();
  for (const commit of commits) {
    const metadata = restoreMetadata(commit);
    if (metadata && metadata.parent !== "none") {
      restoredParents.set(metadata.target, metadata.parent);
    }
  }

  const unwrapRestore = (start: string): string => {
    const seen = new Set<string>();
    let current = start;
    while (!seen.has(current)) {
      seen.add(current);
      const commit = byHash.get(current);
      const target = commit && restoredTarget(commit, commits);
      if (!target || target === current) break;
      current = target;
    }
    return current;
  };

  const currentRef = unwrapRestore(head.commitHash);
  const undoRefs: string[] = [];
  const seen = new Set([currentRef]);
  let next = byHash.get(currentRef)?.parents[0] ?? restoredParents.get(currentRef);
  while (next) {
    const logicalNext = unwrapRestore(next);
    if (seen.has(logicalNext)) break;
    undoRefs.push(logicalNext);
    seen.add(logicalNext);
    next = byHash.get(logicalNext)?.parents[0] ?? restoredParents.get(logicalNext);
  }
  return { currentRef, undoRefs };
}

function restoredTarget(
  commit: TimelineCommit,
  commits: TimelineCommit[],
): string | null {
  const metadata = restoreMetadata(commit);
  if (metadata) return metadata.target;

  const short = commit.header?.startsWith(RESTORE_HEADER_PREFIX)
    ? commit.header.slice(RESTORE_HEADER_PREFIX.length).trim()
    : null;
  if (!short) return null;
  return commits.find((candidate) =>
    candidate.commitHash !== commit.commitHash &&
    stripHashPrefix(candidate.commitHash).startsWith(short) &&
    (!commit.timelineHash || candidate.timelineHash === commit.timelineHash)
  )?.commitHash ?? null;
}

function restoreMetadata(
  commit: TimelineCommit,
): { target: string; parent: string } | null {
  const header = commit.header;
  const short = header?.startsWith(RESTORE_HEADER_PREFIX)
    ? header.slice(RESTORE_HEADER_PREFIX.length).trim()
    : null;
  const lines = commit.fullMessage?.split("\n");
  if (
    !header || !short || !lines ||
    lines[0] !== header ||
    lines[1] !== "" ||
    !lines[2]?.startsWith(`Agent reasoning: ${RESTORED_REF_MARKER} `) ||
    !lines[3]?.startsWith(`${RESTORED_PARENT_MARKER} `) ||
    lines[4] !== "" ||
    lines.slice(5).join("\n") !== RESTORE_REASON
  ) return null;

  const target = lines[2].slice(`Agent reasoning: ${RESTORED_REF_MARKER} `.length);
  const parent = lines[3].slice(`${RESTORED_PARENT_MARKER} `.length);
  if (
    !isCommitHash(target) ||
    !stripHashPrefix(target).startsWith(short) ||
    (parent !== "none" && !isCommitHash(parent))
  ) return null;
  return { target, parent };
}

function isCommitHash(value: string): boolean {
  return /^sha256:[0-9a-f]{8,64}$/.test(value);
}

function stripHashPrefix(hash: string): string {
  return hash.startsWith("sha256:") ? hash.slice("sha256:".length) : hash;
}
