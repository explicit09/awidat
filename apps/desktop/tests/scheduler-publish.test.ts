import { strict as assert } from "node:assert";

import {
  buildSchedulerCadenceSlots,
  buildSchedulerMetadataProfile,
  schedulerMetadataFieldConfig,
  buildSchedulerMetadata,
  loadSchedulerAccounts,
  mergeSchedulerMetadataEdit,
  mergeSchedulerPublishResult,
  mergeSchedulerPublishResults,
  publishSchedulerPostToAccounts,
  publishSchedulerPostViaServer,
  schedulerMetadataControlProvider,
  schedulerPublishableEntries,
  updateSchedulerTargetMetadata,
  validateSchedulerMetadataForAccounts,
} from "../src/app/scheduler/schedulerPublish.ts";
import type { RenderQueueEntry } from "../src/app/renderQueue.ts";

const finishedEntry: RenderQueueEntry = {
  id: "queue_1",
  targetId: "youtube",
  label: "Launch clip",
  kind: "video_master",
  status: "done",
  enqueuedAt: 1_000,
  completedAt: 2_000,
  jobId: "render_1",
  outputPath: "/tmp/launch.mp4",
  uploadMetadata: {
    youtube: {
      title: "Launch title",
      description: "Launch description",
      tags: ["montage", "launch"],
      visibility: "unlisted",
      scheduledAt: 3_000,
      thumbnailPath: "/tmp/thumb.jpg",
    },
  },
};

assert.deepEqual(schedulerPublishableEntries([
  finishedEntry,
  { ...finishedEntry, id: "running", status: "running", outputPath: undefined },
]).map((entry) => entry.id), ["queue_1"]);

const schedulerAccounts = await loadSchedulerAccounts(async (command) => {
  assert.equal(command, "social_accounts");
  return [
    {
      id: "acct_tiktok",
      provider: "tiktok",
      capabilities: { uploadVideo: true },
    },
  ];
});
assert.equal(schedulerAccounts.accounts.length, 1);
assert.equal(schedulerAccounts.error, null);

const failedSchedulerAccounts = await loadSchedulerAccounts(async () => {
  throw new Error("server offline");
});
assert.deepEqual(failedSchedulerAccounts, {
  accounts: [],
  error: "server offline",
});

const editedMetadata = buildSchedulerMetadata({
  title: "Edited title",
  description: "Edited description",
  tagsInput: "montage, edited, montage",
  thumbnailPath: "  /tmp/edited.jpg  ",
  privacy: "public",
  scheduledFor: 4_000,
});
assert.deepEqual(editedMetadata, {
  title: "Edited title",
  description: "Edited description",
  tags: ["montage", "edited"],
  visibility: "public",
  scheduledAt: 4_000,
  thumbnailPath: "/tmp/edited.jpg",
});
assert.deepEqual(
  mergeSchedulerMetadataEdit(
    {
      ...finishedEntry,
      uploadMetadata: {
        youtube: {
          title: "Old YouTube",
          description: "",
          tags: [],
          visibility: "private",
        },
        tiktok: {
          title: "Keep TikTok",
          description: "Keep this",
          tags: ["keep"],
          visibility: "private",
        },
      },
    },
    "youtube",
    editedMetadata,
  ),
  {
    youtube: editedMetadata,
    tiktok: {
      title: "Keep TikTok",
      description: "Keep this",
      tags: ["keep"],
      visibility: "private",
    },
  },
);
assert.deepEqual(
  buildSchedulerCadenceSlots(
    [
      { id: "acct_youtube", provider: "youtube" },
      { id: "acct_tiktok", provider: "tiktok" },
      { id: "acct_instagram", provider: "instagram" },
    ],
    4_000,
    15,
  ).map((slot) => `${slot.provider}:${slot.scheduledFor}`),
  ["youtube:4000", "tiktok:4900", "instagram:5800"],
);
assert.deepEqual(
  validateSchedulerMetadataForAccounts(
    [
      { id: "acct_instagram", provider: "instagram" },
      { id: "acct_youtube", provider: "youtube" },
    ],
    {
      title: "",
      description: "Short caption",
      tagsInput: "",
      thumbnailPath: "",
      privacy: "private",
      scheduledFor: 4_000,
    },
  ).map((error) => `${error.provider}:${error.code}`),
  ["youtube:title.required"],
);
assert.deepEqual(schedulerMetadataFieldConfig("instagram"), {
  showTitle: false,
  titleLabel: "Title",
  showDescription: true,
  descriptionLabel: "Caption",
  descriptionPlaceholder: "Caption shown under the post",
  showTags: true,
  showThumbnail: true,
  visibilityOptions: null,
});
assert.deepEqual(schedulerMetadataFieldConfig("twitter_x"), {
  showTitle: true,
  titleLabel: "Post text",
  showDescription: false,
  descriptionLabel: "Description",
  descriptionPlaceholder: "Long-form description",
  showTags: false,
  showThumbnail: false,
  visibilityOptions: null,
});
assert.deepEqual(
  schedulerMetadataFieldConfig("tiktok").visibilityOptions?.map(
    (option) => `${option.value}:${option.label}`,
  ),
  ["private:Private", "unlisted:Friends only", "public:Public"],
);
assert.equal(
  schedulerMetadataFieldConfig("tiktok").showDescription,
  false,
  "TikTok Direct Post sends the title as caption and should not expose an unused description field",
);
assert.equal(schedulerMetadataFieldConfig("youtube").descriptionLabel, "Description");
assert.equal(
  schedulerMetadataControlProvider([
    { id: "acct_instagram", provider: "instagram" },
    { id: "acct_youtube", provider: "youtube" },
  ]),
  "youtube",
);
assert.equal(
  schedulerMetadataControlProvider([
    { id: "acct_instagram", provider: "instagram" },
    { id: "acct_x", provider: "twitter_x" },
  ]),
  "twitter_x",
);
assert.equal(
  schedulerMetadataControlProvider([
    { id: "acct_instagram", provider: "instagram" },
  ]),
  "instagram",
);
assert.deepEqual(
  buildSchedulerMetadataProfile({
    provider: "instagram",
    renderLabel: "Launch clip",
    scheduledFor: 4_000,
  }),
  {
    title: "",
    description: "Launch clip",
    tagsInput: "",
    thumbnailPath: "",
    privacy: "private",
    scheduledFor: 4_000,
  },
);
assert.deepEqual(
  buildSchedulerMetadataProfile({
    provider: "tiktok",
    renderLabel: "Launch clip",
    scheduledFor: 4_000,
  }),
  {
    title: "Launch clip",
    description: "",
    tagsInput: "",
    thumbnailPath: "",
    privacy: "private",
    scheduledFor: 4_000,
  },
);

const updateTargetCalls: { command: string; args?: Record<string, unknown> }[] = [];
await updateSchedulerTargetMetadata({
  provider: "youtube",
  targetId: "target_1",
  title: "Edited title",
  description: "Edited description",
  tagsInput: "edited, launch",
  thumbnailPath: "/tmp/edited.jpg",
  privacy: "public",
  scheduledFor: 5_000,
  nowSeconds: () => 4_500,
  invoke: async (command, args) => {
    updateTargetCalls.push({ command, args });
    if (command === "social_update_target") {
      return { id: "target_1", validationState: "pending" };
    }
    if (command === "social_validate_target") {
      return { id: "target_1", validationState: "valid" };
    }
    throw new Error(`unexpected update command ${command}`);
  },
});
assert.deepEqual(updateTargetCalls, [
  {
    command: "social_update_target",
    args: {
      args: {
        targetId: "target_1",
        platformFields: {
          privacy: "public",
          title: "Edited title",
          description: "Edited description",
          tags: ["edited", "launch"],
          thumbnailRef: "file:///tmp/edited.jpg",
        },
        scheduledFor: 5_000,
        now: 4_500,
      },
    },
  },
  {
    command: "social_validate_target",
    args: { targetId: "target_1", now: 4_500 },
  },
]);

const calls: { command: string; args?: Record<string, unknown> }[] = [];

async function invoke<T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  calls.push({ command, args });
  if (command === "social_bind_target") {
    const bindArgs = args?.args as Record<string, unknown>;
    assert.equal(bindArgs.targetId, "target_1");
    assert.equal(bindArgs.campaignId, "scheduler-queue_1");
    assert.equal(bindArgs.variantId, "youtube-queue_1-job_1");
    assert.equal(bindArgs.connectedAccountId, "acct_youtube");
    assert.equal(bindArgs.scheduledFor, 3_000);
    assert.equal(bindArgs.now, 2_500);
    assert.deepEqual(bindArgs.platformFields, {
      privacy: "unlisted",
      title: "Launch title",
      description: "Launch description",
      tags: ["montage", "launch"],
      thumbnailRef: "file:///tmp/thumb.jpg",
    });
    return { id: "target_1" } as T;
  }
  if (command === "social_validate_target") {
    assert.deepEqual(args, { targetId: "target_1", now: 2_500 });
    return { validationState: "valid" } as T;
  }
  if (command === "social_schedule_target") {
    assert.deepEqual(args?.args, {
      targetId: "target_1",
      jobId: "job_1",
      artifactRef: "",
      createdBy: "desktop-scheduler",
      now: 2_500,
    });
    return { id: "job_1", status: "scheduled" } as T;
  }
  if (command === "social_upload_artifact") {
    assert.deepEqual(args, {
      jobId: "job_1",
      filePath: "/tmp/launch.mp4",
    });
    return undefined as T;
  }
  throw new Error(`unexpected command ${command}`);
}

const result = await publishSchedulerPostViaServer({
  entry: finishedEntry,
  account: {
    id: "acct_youtube",
    provider: "youtube",
    capabilities: { uploadVideo: true },
  },
  title: "Launch title",
  description: "Launch description",
  tagsInput: "montage, launch",
  thumbnailPath: "/tmp/thumb.jpg",
  privacy: "unlisted",
  scheduledFor: 3_000,
  invoke,
  idFactory: (prefix) => `${prefix}_1`,
  nowSeconds: () => 2_500,
});

assert.equal(result.provider, "youtube");
assert.equal(result.jobId, "job_1");
assert.deepEqual(result.uploadState, {
  state: "scheduled",
  job_id: "job_1",
  target_id: "target_1",
});
assert.deepEqual(result.metadata, {
  title: "Launch title",
  description: "Launch description",
  tags: ["montage", "launch"],
  visibility: "unlisted",
  scheduledAt: 3_000,
  thumbnailPath: "/tmp/thumb.jpg",
});

const tiktokBindCalls: { command: string; args?: Record<string, unknown> }[] = [];
await publishSchedulerPostViaServer({
  entry: finishedEntry,
  account: {
    id: "acct_tiktok",
    provider: "tiktok",
    capabilities: { uploadVideo: true },
  },
  title: "TikTok launch",
  description: "TikTok caption",
  tagsInput: "",
  thumbnailPath: "",
  privacy: "private",
  scheduledFor: 3_000,
  tiktokInteractions: {
    disableDuet: true,
    disableComment: true,
    disableStitch: true,
  },
  invoke: async (command, args) => {
    tiktokBindCalls.push({ command, args });
    if (command === "social_validate_target") return { validationState: "valid" };
    if (command === "social_schedule_target") return { id: "job_tiktok", status: "scheduled" };
    if (command === "social_upload_artifact") return undefined;
    return { id: "target_tiktok" };
  },
  idFactory: (prefix) => `${prefix}_tiktok`,
  nowSeconds: () => 2_500,
});
assert.deepEqual(
  (tiktokBindCalls[0].args?.args as Record<string, unknown>).platformFields,
  {
    privacy: "private",
    title: "TikTok launch",
    disableDuet: true,
    disableComment: true,
    disableStitch: true,
  },
);

const eventResult = await publishSchedulerPostViaServer({
  entry: finishedEntry,
  account: {
    id: "acct_youtube",
    provider: "youtube",
    capabilities: { uploadVideo: true },
  },
  title: "Launch title",
  description: "Launch description",
  tagsInput: "",
  thumbnailPath: "",
  privacy: "private",
  scheduledFor: 3_000,
  invoke: async (command, args) => {
    if (command === "social_validate_target") return { validationState: "valid" };
    if (command === "social_schedule_target") {
      const scheduleArgs = args?.args as Record<string, unknown>;
      return { id: scheduleArgs.jobId, status: "scheduled" };
    }
    if (command === "social_upload_artifact") {
      return {
        id: "job_events",
        status: "scheduled",
        events: [
          {
            id: "event_1",
            eventType: "artifact_uploaded",
            message: "Upload artifact attached",
            metadata: {},
            createdAt: 2_510,
          },
        ],
      };
    }
    return { id: "target_1" };
  },
  idFactory: (prefix) => `${prefix}_events`,
  nowSeconds: () => 2_500,
});
assert.deepEqual(eventResult.uploadState, {
  state: "scheduled",
  job_id: "job_events",
  target_id: "target_events",
  events: [
    {
      id: "event_1",
      eventType: "artifact_uploaded",
      message: "Upload artifact attached",
      metadata: {},
      createdAt: 2_510,
    },
  ],
});
assert.deepEqual(
  mergeSchedulerPublishResult(
    {
      ...finishedEntry,
      uploadTargets: ["tiktok"],
      uploadStates: {
        tiktok: {
          state: "published",
          remote_url: "https://tiktok.example/post/1",
          remote_id: "tt_1",
        },
      },
      publishedUrls: {
        tiktok: "https://tiktok.example/post/1",
      },
      uploadMetadata: {
        tiktok: {
          title: "TikTok",
          description: "",
          tags: [],
          visibility: "private",
        },
      },
    },
    result,
  ),
  {
    uploadTargets: ["tiktok", "youtube"],
    uploadStates: {
      tiktok: {
        state: "published",
        remote_url: "https://tiktok.example/post/1",
        remote_id: "tt_1",
      },
      youtube: { state: "scheduled", job_id: "job_1", target_id: "target_1" },
    },
    publishedUrls: {
      tiktok: "https://tiktok.example/post/1",
    },
    uploadMetadata: {
      tiktok: {
        title: "TikTok",
        description: "",
        tags: [],
        visibility: "private",
      },
      youtube: result.metadata,
    },
  },
);

assert.deepEqual(
  mergeSchedulerPublishResults(finishedEntry, [
    {
      provider: "youtube",
      jobId: "job_youtube",
      uploadState: { state: "scheduled", job_id: "job_youtube" },
      metadata: {
        title: "YouTube",
        description: "",
        tags: [],
        visibility: "private",
      },
    },
    {
      provider: "tiktok",
      jobId: "job_tiktok",
      uploadState: { state: "scheduled", job_id: "job_tiktok" },
      metadata: {
        title: "TikTok",
        description: "",
        tags: [],
        visibility: "private",
      },
    },
  ]),
  {
    uploadTargets: ["youtube", "tiktok"],
    uploadStates: {
      youtube: { state: "scheduled", job_id: "job_youtube" },
      tiktok: { state: "scheduled", job_id: "job_tiktok" },
    },
    publishedUrls: {},
    uploadMetadata: {
      youtube: {
        title: "YouTube",
        description: "",
        tags: [],
        visibility: "private",
      },
      tiktok: {
        title: "TikTok",
        description: "",
        tags: [],
        visibility: "private",
      },
    },
  },
);
assert.deepEqual(
  calls.map((call) => call.command),
  [
    "social_bind_target",
    "social_validate_target",
    "social_schedule_target",
    "social_upload_artifact",
  ],
);

const multiCalls: { command: string; args?: Record<string, unknown> }[] = [];
let multiIdSeq = 0;
async function multiInvoke<T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  multiCalls.push({ command, args });
  if (command === "social_bind_target") return { id: "target" } as T;
  if (command === "social_validate_target") return { validationState: "valid" } as T;
  if (command === "social_schedule_target") {
    const scheduleArgs = args?.args as Record<string, unknown>;
    return { id: scheduleArgs.jobId, status: "scheduled" } as T;
  }
  if (command === "social_upload_artifact") return undefined as T;
  throw new Error(`unexpected command ${command}`);
}

const multiResults = await publishSchedulerPostToAccounts({
  entry: finishedEntry,
  accounts: [
    {
      id: "acct_youtube",
      provider: "youtube",
      capabilities: { uploadVideo: true },
    },
    {
      id: "acct_tiktok",
      provider: "tiktok",
      capabilities: { uploadVideo: true },
    },
  ],
  title: "Multi title",
  description: "Multi description",
  tagsInput: "montage",
  thumbnailPath: "",
  privacy: "private",
  scheduledFor: 4_000,
  cadenceMinutes: 15,
  invoke: multiInvoke,
  idFactory: (prefix) => `${prefix}_${++multiIdSeq}`,
  nowSeconds: () => 3_500,
});
assert.deepEqual(
  multiResults.map((result) => result.provider),
  ["youtube", "tiktok"],
);
assert.deepEqual(
  multiCalls.filter((call) => call.command === "social_schedule_target").map((call) => {
    const args = call.args?.args as Record<string, unknown>;
    return args.jobId;
  }),
  ["job_2", "job_4"],
);
assert.deepEqual(
  multiCalls.filter((call) => call.command === "social_bind_target").map((call) => {
    const args = call.args?.args as Record<string, unknown>;
    return args.scheduledFor;
  }),
  [4_000, 4_900],
);
assert.deepEqual(
  multiResults.map((result) => result.metadata.scheduledAt),
  [4_000, 4_900],
);

const blockedCalls: string[] = [];
await assert.rejects(
  () =>
    publishSchedulerPostToAccounts({
      entry: finishedEntry,
      accounts: [
        {
          id: "acct_youtube",
          provider: "youtube",
          capabilities: { uploadVideo: true },
        },
      ],
      title: "",
      description: "",
      tagsInput: "",
      thumbnailPath: "",
      privacy: "private",
      scheduledFor: 4_000,
      invoke: async (command) => {
        blockedCalls.push(command);
        return {} as never;
      },
    }),
  /Not valid: YouTube title required/,
);
assert.deepEqual(blockedCalls, []);

async function invalidInvoke<T>(
  command: string,
): Promise<T> {
  if (command === "social_bind_target") return { id: "target_1" } as T;
  if (command === "social_validate_target") {
    return {
      validationState: "invalid",
      validationReasons: ["title.required"],
    } as T;
  }
  throw new Error(`unexpected command ${command}`);
}

await assert.rejects(
  () =>
    publishSchedulerPostViaServer({
      entry: finishedEntry,
      account: {
        id: "acct_youtube",
        provider: "youtube",
        capabilities: { uploadVideo: true },
      },
      title: "",
      description: "",
      tagsInput: "",
      thumbnailPath: "",
      privacy: "private",
      scheduledFor: 3_000,
      invoke: invalidInvoke,
      idFactory: (prefix) => `${prefix}_1`,
      nowSeconds: () => 2_500,
    }),
  /Not valid: title required/,
);

console.log("scheduler-publish: OK");
