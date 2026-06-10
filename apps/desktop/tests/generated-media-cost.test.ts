import { strict as assert } from "node:assert";
import { generatedMediaCostLabel } from "../src/media/generatedMediaCost.ts";

assert.equal(
  generatedMediaCostLabel({ cost_actual_usd: 0.37, cost_estimate_usd: 0.42 }),
  "Actual cost $0.37",
);
assert.equal(
  generatedMediaCostLabel({ cost_estimate_usd: 0.42 }),
  "Estimated cost $0.42",
);
assert.equal(generatedMediaCostLabel({}), "cost unknown");

console.log("generated-media-cost: OK");
