import { strict as assert } from "node:assert";
import { colorPreviewCssFilter } from "../src/media/colorPreviewFilter.ts";

const rest = {
  exposure_ev: 0,
  contrast: 1,
  saturation: 1,
  temperature: 0,
  tint: 0,
  shadows: 0,
  highlights: 0,
};

// No correction / resting values → empty string (clears stale filters).
{
  assert.equal(colorPreviewCssFilter(null), "");
  assert.equal(colorPreviewCssFilter(undefined), "");
  assert.equal(colorPreviewCssFilter(rest), "");
}

// Exposure is photographic stops: +1 EV doubles brightness.
{
  assert.equal(colorPreviewCssFilter({ ...rest, exposure_ev: 1 }), "brightness(2)");
  assert.equal(colorPreviewCssFilter({ ...rest, exposure_ev: -1 }), "brightness(0.5)");
}

// Contrast and saturation map 1:1.
{
  assert.equal(colorPreviewCssFilter({ ...rest, contrast: 1.5 }), "contrast(1.5)");
  assert.equal(colorPreviewCssFilter({ ...rest, saturation: 0 }), "saturate(0)");
}

// Combined corrections compose in stable order.
{
  assert.equal(
    colorPreviewCssFilter({ ...rest, exposure_ev: 1, contrast: 1.2, saturation: 0.8 }),
    "brightness(2) contrast(1.2) saturate(0.8)",
  );
}

// Render-only fields (temperature/tint/shadows/highlights) do not
// produce a misleading CSS approximation.
{
  assert.equal(
    colorPreviewCssFilter({ ...rest, temperature: 0.8, tint: -0.5, shadows: 0.4, highlights: -0.3 }),
    "",
  );
}

// Out-of-range and garbage inputs clamp/degrade safely.
{
  assert.equal(colorPreviewCssFilter({ ...rest, exposure_ev: 99 }), "brightness(16)");
  assert.equal(colorPreviewCssFilter({ ...rest, contrast: -5 }), "contrast(0)");
  assert.equal(colorPreviewCssFilter({ ...rest, saturation: NaN }), "");
}

console.log("color-preview-filter: all assertions passed");
