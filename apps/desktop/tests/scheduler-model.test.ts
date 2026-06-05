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

const actionNeededEntry: RenderQueueEntry = {
  ...entry,
  id: "queue_action_needed_reasons",
  uploadTargets: ["server", "scope", "reauth", "text", "timeout"],
  uploadMetadata: {
    server: {
      title: "Server fallback action",
      description: "",
      tags: [],
      visibility: "private",
      scheduledAt: 4_000,
    },
    scope: {
      title: "Missing scope action",
      description: "",
      tags: [],
      visibility: "private",
      scheduledAt: 4_001,
    },
    reauth: {
      title: "Provider reauth action",
      description: "",
      tags: [],
      visibility: "private",
      scheduledAt: 4_002,
    },
    text: {
      title: "Human reauth action",
      description: "",
      tags: [],
      visibility: "private",
      scheduledAt: 4_003,
    },
    timeout: {
      title: "Ordinary failure",
      description: "",
      tags: [],
      visibility: "private",
      scheduledAt: 4_004,
    },
  },
  uploadStates: {
    server: {
      state: "failed",
      reason: "server publish requires_action",
      job_id: "job_server_action",
    },
    scope: {
      state: "failed",
      reason: "missing_scope",
      job_id: "job_scope_action",
    },
    reauth: {
      state: "failed",
      reason: "youtube_reauth_required",
      job_id: "job_reauth_action",
    },
    text: {
      state: "failed",
      reason: "token refresh failed; account needs reauth",
      job_id: "job_text_action",
    },
    timeout: {
      state: "failed",
      reason: "upload timed out",
      job_id: "job_timeout_failure",
    },
  },
};

assert.deepEqual(
  deriveSchedulerPosts([actionNeededEntry], 1_700).map(
    (post) => `${post.provider}:${post.status}:${post.failureReason}`,
  ),
  [
    "server:requires_action:server publish requires_action",
    "scope:requires_action:missing_scope",
    "reauth:requires_action:youtube_reauth_required",
    "text:requires_action:token refresh failed; account needs reauth",
    "timeout:failed:upload timed out",
  ],
);
assert.equal(schedulerStatusLabel("requires_action"), "Action needed");
assert.equal(formatSchedulerTime(1_800, "UTC"), "1970-01-01 00:30 UTC");

console.log("scheduler-model: OK");
