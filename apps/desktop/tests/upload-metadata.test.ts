// Tests for the W5.A3 per-target upload metadata validation logic
// and the `useUploadMetadata` store actions.
//
// We don't mount React — the validation surface (`validateMetadata`)
// is pure, and the store actions are exercised directly through the
// same pattern `render-queue-upload.test.ts` uses.

import { strict as assert } from "node:assert";

import {
  defaultMetadata,
  PLATFORM_LIMITS,
  useUploadMetadata,
  validateMetadata,
  visibilityOptionsFor,
  type UploadMetadata,
} from "../src/state/uploadMetadata.ts";

function resetStore(): void {
  useUploadMetadata.setState({ byKey: {} });
}

// ---- defaultMetadata seeds title from the label ----
{
  const md = defaultMetadata("My Render");
  assert.equal(md.title, "My Render");
  assert.equal(md.description, "");
  assert.deepEqual(md.tags, []);
  assert.equal(md.visibility, "private");
  assert.equal(md.scheduledAt, undefined);
  assert.equal(md.thumbnailPath, undefined);
}

// ---- title.required fires on empty title (YouTube / TikTok) ----
{
  for (const provider of ["youtube", "tiktok"]) {
    const md: UploadMetadata = { ...defaultMetadata(""), title: "" };
    const errors = validateMetadata(provider, md);
    assert.ok(
      errors.find((e) => e.code === "title.required"),
      `${provider} should require a title`,
    );
  }
}

// ---- Instagram has no title field; caption is the validated text ----
{
  // Empty title is fine on Instagram.
  const ok = validateMetadata("instagram", { ...defaultMetadata(""), title: "" });
  assert.equal(
    ok.find((e) => e.code === "title.required"),
    undefined,
    "Instagram doesn't require a title",
  );
  // But caption > 2200 chars is rejected.
  const caption = "x".repeat(2201);
  const tooLong = validateMetadata("instagram", {
    ...defaultMetadata("anything"),
    description: caption,
  });
  assert.ok(
    tooLong.find((e) => e.code === "caption.too_long"),
    "Instagram caption > 2200 chars must error",
  );
}

// ---- YouTube title <= 100 chars ----
{
  const ok = validateMetadata("youtube", {
    ...defaultMetadata(""),
    title: "x".repeat(100),
  });
  assert.equal(
    ok.find((e) => e.code === "title.too_long"),
    undefined,
    "100-char title is the limit, not exceeding",
  );
  const tooLong = validateMetadata("youtube", {
    ...defaultMetadata(""),
    title: "x".repeat(101),
  });
  assert.ok(
    tooLong.find((e) => e.code === "title.too_long"),
    "YouTube title > 100 must error",
  );
}

// ---- YouTube description <= 5000 chars ----
{
  const tooLong = validateMetadata("youtube", {
    ...defaultMetadata("ok title"),
    description: "x".repeat(5001),
  });
  assert.ok(
    tooLong.find((e) => e.code === "description.too_long"),
    "YouTube description > 5000 must error",
  );
}

// ---- YouTube tags total <= 500 chars ----
{
  const okTags = Array(10).fill("x".repeat(49)); // 490
  const ok = validateMetadata("youtube", {
    ...defaultMetadata("ok title"),
    tags: okTags,
  });
  assert.equal(
    ok.find((e) => e.code === "tags.too_long"),
    undefined,
  );
  const overTags = Array(11).fill("x".repeat(50)); // 550
  const overflow = validateMetadata("youtube", {
    ...defaultMetadata("ok title"),
    tags: overTags,
  });
  assert.ok(
    overflow.find((e) => e.code === "tags.too_long"),
    "YouTube tags > 500 chars total must error",
  );
}

// ---- TikTok limits: title <= 150, description <= 4000 ----
{
  const tooLongTitle = validateMetadata("tiktok", {
    ...defaultMetadata(""),
    title: "x".repeat(151),
  });
  assert.ok(tooLongTitle.find((e) => e.code === "title.too_long"));
  const tooLongDesc = validateMetadata("tiktok", {
    ...defaultMetadata("ok"),
    description: "x".repeat(4001),
  });
  assert.ok(tooLongDesc.find((e) => e.code === "description.too_long"));
}

// ---- schedule.in_past fires for old timestamps ----
{
  // 1 hour ago in epoch seconds.
  const past = Math.floor(Date.now() / 1000) - 3600;
  const errs = validateMetadata("youtube", {
    ...defaultMetadata("ok"),
    scheduledAt: past,
  });
  assert.ok(errs.find((e) => e.code === "schedule.in_past"));
}

// ---- schedule.invalid fires for bogus timestamps ----
{
  const errs = validateMetadata("youtube", {
    ...defaultMetadata("ok"),
    scheduledAt: 0,
  });
  assert.ok(errs.find((e) => e.code === "schedule.invalid"));
}

// ---- visibilityOptionsFor: Instagram returns null, TikTok labels differ ----
{
  assert.equal(
    visibilityOptionsFor("instagram"),
    null,
    "Instagram has no visibility toggle",
  );
  const tikTok = visibilityOptionsFor("tiktok");
  assert.ok(tikTok);
  assert.ok(
    tikTok?.find((o) => o.label === "Friends only"),
    "TikTok has Friends only label",
  );
  const yt = visibilityOptionsFor("youtube");
  assert.ok(yt);
  assert.ok(yt?.find((o) => o.label === "Unlisted"));
}

// ---- platform limits export shape ----
{
  assert.equal(PLATFORM_LIMITS.youtube.titleMax, 100);
  assert.equal(PLATFORM_LIMITS.youtube.descriptionMax, 5000);
  assert.equal(PLATFORM_LIMITS.youtube.tagsTotalCharsMax, 500);
  assert.equal(PLATFORM_LIMITS.tiktok.titleMax, 150);
  assert.equal(PLATFORM_LIMITS.tiktok.descriptionMax, 4000);
  assert.equal(PLATFORM_LIMITS.instagram.captionMax, 2200);
}

// ---- useUploadMetadata.set + get round-trips by (jobId, provider) ----
{
  resetStore();
  const md = {
    ...defaultMetadata("Custom"),
    description: "hello",
    tags: ["a", "b"],
    visibility: "public" as const,
  };
  useUploadMetadata.getState().set("job-1", "youtube", md);
  const fetched = useUploadMetadata.getState().get("job-1", "youtube", "fallback");
  assert.deepEqual(fetched, md);
  // Missing (jobId, provider) → fallback to defaults built from label.
  const missing = useUploadMetadata.getState().get("job-1", "tiktok", "fallback");
  assert.equal(missing.title, "fallback");
}

// ---- forgetJob drops entries for the matching jobId ----
{
  resetStore();
  const store = useUploadMetadata.getState();
  store.set("job-A", "youtube", defaultMetadata("yt"));
  store.set("job-A", "tiktok", defaultMetadata("tt"));
  store.set("job-B", "youtube", defaultMetadata("other"));
  store.forgetJob("job-A");
  const state = useUploadMetadata.getState();
  assert.equal(state.byKey["job-A::youtube"], undefined);
  assert.equal(state.byKey["job-A::tiktok"], undefined);
  // job-B untouched.
  assert.ok(state.byKey["job-B::youtube"]);
}

console.log("upload-metadata: OK");
