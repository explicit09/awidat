import { useMode } from "../state/mode";
import type { IndexRailProProps as IndexRailProps } from "./IndexRailPro";
import { IndexRailCreator } from "./IndexRailCreator";
import { IndexRailPro } from "./IndexRailPro";

export type { IndexRailProProps as IndexRailProps } from "./IndexRailPro";

export function IndexRail(props: IndexRailProps) {
  const mode = useMode((s) => s.mode);
  return mode === "creator" ? (
    <IndexRailCreator {...props} />
  ) : (
    <IndexRailPro {...props} />
  );
}
