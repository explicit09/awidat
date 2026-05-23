// Render a `simple-icons` SVG as a React component.
//
// The simple-icons package ships each icon as `{ title, path, hex }`
// where `path` is the `<path d>` data inside a 0 0 24 24 viewBox.
// We render it as a self-contained SVG so callers can size + color
// it like any lucide icon (size via className h-N w-N; color via
// the brand hex when `tinted` is true, or via currentColor inherited
// from the surrounding text color otherwise).
//
// Trademark note: simple-icons distributes the official brand marks
// under CC0 but explicitly disclaims trademark license. The package
// docs note that *nominative use* in UI ("this software exports to
// YouTube") is generally accepted; sponsorship / endorsement use is
// not. The Deliver page falls squarely in the nominative bucket.

import type { CSSProperties } from "react";

export type SimpleIconShape = {
  title: string;
  /** The single SVG path data. */
  path: string;
  /** Brand hex without `#`, e.g. `"FF0000"` for YouTube red. */
  hex: string;
};

export type BrandIconProps = {
  icon: SimpleIconShape;
  /** When true, fill = brand hex; otherwise fill = currentColor.
   *  Selected target rows pop the brand color so they read like
   *  Resolve's tiles. */
  tinted?: boolean;
  className?: string;
  /** Inline style override; takes precedence over tinted. */
  style?: CSSProperties;
};

export function BrandIcon({
  icon,
  tinted = false,
  className,
  style,
}: BrandIconProps) {
  const fill = style?.color ?? (tinted ? `#${icon.hex}` : "currentColor");
  return (
    <svg
      role="img"
      aria-label={icon.title}
      viewBox="0 0 24 24"
      xmlns="http://www.w3.org/2000/svg"
      className={className}
      style={style}
    >
      <path d={icon.path} fill={fill} />
    </svg>
  );
}
