import type { CSSProperties } from "react";

export type ProgramFrameRect = {
  left: number;
  top: number;
  width: number;
  height: number;
};

export function containedProgramFrame(
  containerWidth: number,
  containerHeight: number,
  mediaWidth: number,
  mediaHeight: number,
): ProgramFrameRect | null {
  if (
    !isPositiveFinite(containerWidth) ||
    !isPositiveFinite(containerHeight) ||
    !isPositiveFinite(mediaWidth) ||
    !isPositiveFinite(mediaHeight)
  ) {
    return null;
  }

  const containerRatio = containerWidth / containerHeight;
  const mediaRatio = mediaWidth / mediaHeight;
  if (containerRatio > mediaRatio) {
    const width = containerHeight * mediaRatio;
    return {
      left: (containerWidth - width) / 2,
      top: 0,
      width,
      height: containerHeight,
    };
  }

  const height = containerWidth / mediaRatio;
  return {
    left: 0,
    top: (containerHeight - height) / 2,
    width: containerWidth,
    height,
  };
}

export function programFrameStyle(rect: ProgramFrameRect | null): CSSProperties {
  if (!rect) {
    return { position: "absolute", inset: 0 };
  }
  return {
    position: "absolute",
    left: `${rect.left}px`,
    top: `${rect.top}px`,
    width: `${rect.width}px`,
    height: `${rect.height}px`,
  };
}

function isPositiveFinite(value: number): boolean {
  return Number.isFinite(value) && value > 0;
}

/**
 * Human aspect-ratio label for a pixel size: exact small ratios come
 * out as "16:9" / "9:16" / "4:3"; everything else as "1.48:1".
 */
export function aspectRatioLabel(width: number, height: number): string | null {
  if (!isPositiveFinite(width) || !isPositiveFinite(height)) return null;
  const divisor = gcd(Math.round(width), Math.round(height));
  const w = Math.round(width) / divisor;
  const h = Math.round(height) / divisor;
  if (w <= 32 && h <= 32) return `${w}:${h}`;
  return `${(width / height).toFixed(2).replace(/\.?0+$/, "")}:1`;
}

function gcd(a: number, b: number): number {
  return b === 0 ? a : gcd(b, a % b);
}
