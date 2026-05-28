import { useMode } from "../state/mode";
import type { IndexingDashboardProps } from "./IndexingDashboard";
import { IndexRailPro } from "./IndexRailPro";

// Types from IndexingDashboard are import-type-only, so this file does
// NOT participate in a runtime cycle even though IndexingDashboard.tsx
// re-exports IndexRail as IndexingDashboard.

/**
 * IndexRail — thin selector between Pro and Creator index surfaces.
 *
 * In Pro mode (and currently in Creator until Task 15 lands a dedicated
 * surface), renders the dense IndexRailPro. The full IndexingDashboardProps
 * shape is accepted for source compatibility with the old IndexingDashboard
 * callers — Pro consumes only the subset it needs.
 */
export type IndexRailProps = IndexingDashboardProps;

export function IndexRail(props: IndexRailProps) {
  const mode = useMode((s) => s.mode);
  if (mode === "creator") {
    // Creator surface arrives in Task 15. For now, Pro is the only render path.
    return <IndexRailPro {...toProProps(props)} />;
  }
  return <IndexRailPro {...toProProps(props)} />;
}

function toProProps(props: IndexRailProps) {
  return {
    tasks: props.tasks,
    structurePreview: props.structurePreview,
    indexerConfig: props.indexerConfig,
    ready: props.ready,
    onRefreshIndexers: props.onReviewIndexResults,
    onOpenConfigPath: props.onOpenConfigPath,
  };
}
