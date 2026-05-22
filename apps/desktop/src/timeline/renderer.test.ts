import assert from "node:assert/strict";
import { pathRoundedRect, truncateToWidth } from "./canvasPrimitives.ts";

function measureTextByLength(text: string) {
  return { width: text.length * 10 } as TextMetrics;
}

console.log("# truncateToWidth");
assert.equal(
  truncateToWidth({ measureText: measureTextByLength } as CanvasRenderingContext2D, "interview", 55),
  "inte…",
);
assert.equal(
  truncateToWidth({ measureText: measureTextByLength } as CanvasRenderingContext2D, "clip", 100),
  "clip",
);
console.log("  ok  truncates text only when it exceeds the available width");

console.log("# pathRoundedRect");
const calls: string[] = [];
const ctx = {
  beginPath: () => calls.push("beginPath"),
  moveTo: (x: number, y: number) => calls.push(`moveTo:${x},${y}`),
  lineTo: (x: number, y: number) => calls.push(`lineTo:${x},${y}`),
  quadraticCurveTo: (cpx: number, cpy: number, x: number, y: number) =>
    calls.push(`quadraticCurveTo:${cpx},${cpy},${x},${y}`),
} as unknown as CanvasRenderingContext2D;

pathRoundedRect(ctx, 10, 20, 30, 10, 20);
assert.deepEqual(calls, [
  "beginPath",
  "moveTo:15,20",
  "lineTo:35,20",
  "quadraticCurveTo:40,20,40,25",
  "lineTo:40,25",
  "quadraticCurveTo:40,30,35,30",
  "lineTo:15,30",
  "quadraticCurveTo:10,30,10,25",
  "lineTo:10,25",
  "quadraticCurveTo:10,20,15,20",
]);
console.log("  ok  clamps radius and traces a rounded rectangle path");
