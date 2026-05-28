import { useMode } from "../state/mode";
import type { IndexingDashboardProps } from "./IndexingDashboard";
import { IndexRailCreator } from "./IndexRailCreator";
import { IndexRailPro } from "./IndexRailPro";

// Types from IndexingDashboard are import-type-only, so this file does
// NOT participate in a runtime cycle even though IndexingDashboard.tsx
// re-exports IndexRail as IndexingDashboard.

/**
 * IndexRail — thin selector between Pro and Creator index surfaces.
 *
 * Reads the active mode from the global mode store (Task 6) and renders the
 * matching rail. Both rails accept the same prop shape (Pro's
 * IndexRailProProps); IndexingDashboardProps is the source-compatible
 * superset so callers don't need to migrate state.
 */
export type IndexRailProps = IndexingDashboardProps;

export function IndexRail(props: IndexRailProps) {
  const mode = useMode((s) => s.mode);
  const railProps = toRailProps(props);
  return mode === "creator" ? (
    <IndexRailCreator {...railProps} />
  ) : (
    <IndexRailPro {...railProps} />
  );
}

function toRailProps(props: IndexRailProps) {
  return {
    tasks: props.tasks,
    structurePreview: props.structurePreview,
    indexerConfig: props.indexerConfig,
    ready: props.ready,
    onRefreshIndexers: props.onReviewIndexResults,
    onOpenConfigPath: props.onOpenConfigPath,
  };
}
