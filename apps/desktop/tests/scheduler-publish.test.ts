import { strict as assert } from "node:assert";

import {
  loadSchedulerAccounts,
  mergeSchedulerPublishResult,
  mergeSchedulerPublishResults,
  publishSchedulerPostToAccounts,
  publishSchedulerPostViaServer,
  schedulerPublishableEntries,
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
      tags: ["awidat", "launch"],
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
      tags: ["awidat", "launch"],
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
  tagsInput: "awidat, launch",
  thumbnailPath: "/tmp/thumb.jpg",
  privacy: "unlisted",
  scheduledFor: 3_000,
  invoke,
  idFactory: (prefix) => `${prefix}_1`,
  nowSeconds: () => 2_500,
});

assert.equal(result.provider, "youtube");
assert.equal(result.jobId, "job_1");
assert.deepEqual(result.uploadState, { state: "scheduled", job_id: "job_1" });
assert.deepEqual(result.metadata, {
  title: "Launch title",
  description: "Launch description",
  tags: ["awidat", "launch"],
  visibility: "unlisted",
  scheduledAt: 3_000,
  thumbnailPath: "/tmp/thumb.jpg",
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
      youtube: { state: "scheduled", job_id: "job_1" },
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
  tagsInput: "awidat",
  thumbnailPath: "",
  privacy: "private",
  scheduledFor: 4_000,
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
