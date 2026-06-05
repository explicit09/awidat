import { strict as assert } from "node:assert";

import {
  deriveSchedulerPosts,
  formatSchedulerTime,
  schedulerStatusLabel,
} from "../src/app/scheduler/schedulerModel.ts";
import type { RenderQueueEntry } from "../src/app/renderQueue.ts";

const entry: RenderQueueEntry = {
  id: "queue_1",
  targetId: "youtube_1080",
  label: "Launch clip",
  kind: "video_master",
  status: "done",
  progress: 100,
  enqueuedAt: 1_000_000,
  completedAt: 1_010_000,
  jobId: "render_1",
  outputPath: "/project/renders/launch.mp4",
  uploadTargets: ["youtube", "tiktok"],
  uploadMetadata: {
    youtube: {
      title: "YouTube launch title",
      description: "Long-form description",
      tags: ["awidat"],
      visibility: "public",
      scheduledAt: 1_800,
      thumbnailPath: undefined,
    },
    tiktok: {
      title: "TikTok hook",
      description: "Short caption",
      tags: [],
      visibility: "private",
      scheduledAt: 2_000,
      thumbnailPath: undefined,
    },
  },
  uploadStates: {
    youtube: { state: "scheduled", job_id: "job_yt" },
    tiktok: {
      state: "published",
      remote_url: "https://tiktok.example/post/1",
      remote_id: "tt_1",
    },
  },
  publishedUrls: {
    tiktok: "https://tiktok.example/post/1",
  },
};

const posts = deriveSchedulerPosts([entry], 1_700);

assert.equal(posts.length, 2);
assert.deepEqual(
  posts.map((post) => `${post.provider}:${post.status}:${post.title}`),
  [
    "youtube:scheduled:YouTube launch title",
    "tiktok:published:TikTok hook",
  ],
);
assert.equal(posts[0].scheduledAt, 1_800);
assert.equal(posts[0].jobId, "job_yt");
assert.equal(posts[0].visibility, "public");
assert.equal(posts[1].providerUrl, "https://tiktok.example/post/1");

const targetFilteredEntry: RenderQueueEntry = {
  ...entry,
  id: "queue_targets_filter_states",
  uploadTargets: ["youtube"],
};

const targetFilteredPosts = deriveSchedulerPosts([targetFilteredEntry], 1_700);

assert.deepEqual(
  targetFilteredPosts.map((post) => post.provider),
  ["youtube"],
);

const stateFallbackEntry: RenderQueueEntry = {
  ...entry,
  id: "queue_state_fallback",
  uploadTargets: undefined,
  uploadMetadata: {},
  uploadStates: {
    instagram: { state: "processing", job_id: "job_ig" },
    youtube: { state: "pending" },
  },
  publishedUrls: {},
};

const emptyTargetsFallbackEntry: RenderQueueEntry = {
  ...stateFallbackEntry,
  id: "queue_empty_targets_fallback",
  uploadTargets: [],
};

assert.deepEqual(
  deriveSchedulerPosts([stateFallbackEntry], 1_700).map(
    (post) => `${post.provider}:${post.status}`,
  ),
  ["instagram:processing", "youtube:draft"],
);
assert.deepEqual(
  deriveSchedulerPosts([emptyTargetsFallbackEntry], 1_700).map(
    (post) => post.provider,
  ),
  ["instagram", "youtube"],
);

const unsortedTargetsEntry: RenderQueueEntry = {
  ...entry,
  id: "queue_unsorted_targets",
  uploadTargets: ["late", "tie_z", "early", "tie_a"],
  uploadMetadata: {
    late: {
      title: "Late post",
      description: "",
      tags: [],
      visibility: "private",
      scheduledAt: 3_000,
    },
    tie_z: {
      title: "Zulu tie",
      description: "",
      tags: [],
      visibility: "private",
      scheduledAt: 2_000,
    },
    early: {
      title: "Early post",
      description: "",
      tags: [],
      visibility: "private",
      scheduledAt: 1_000,
    },
    tie_a: {
      title: "Alpha tie",
      description: "",
      tags: [],
      visibility: "private",
      scheduledAt: 2_000,
    },
  },
  uploadStates: {
    late: { state: "pending" },
    tie_z: { state: "pending" },
    early: { state: "pending" },
    tie_a: { state: "pending" },
  },
};

assert.deepEqual(
  deriveSchedulerPosts([unsortedTargetsEntry], 1_700).map(
    (post) => `${post.provider}:${post.scheduledAt}:${post.title}`,
  ),
  [
    "early:1000:Early post",
    "tie_a:2000:Alpha tie",
    "tie_z:2000:Zulu tie",
    "late:3000:Late post",
  ],
);
assert.equal(schedulerStatusLabel("requires_action"), "Action needed");
assert.equal(formatSchedulerTime(1_800, "UTC"), "1970-01-01 00:30 UTC");

console.log("scheduler-model: OK");
