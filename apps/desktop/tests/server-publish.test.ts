import { strict as assert } from "node:assert";

import { publishRenderTargetsViaServer } from "../src/app/serverPublish.ts";

const calls: string[] = [];

async function invoke<T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  calls.push(command);
  if (command === "social_accounts") {
    return [
      {
        id: "acct_yt",
        provider: "youtube",
        capabilities: { uploadVideo: true },
      },
    ] as T;
  }
  if (command === "social_bind_target") {
    const bindArgs = args?.args as {
      scheduledFor?: number;
      now?: number;
      platformFields?: Record<string, unknown>;
    };
    assert.equal(bindArgs.scheduledFor, 1_001);
    assert.equal(bindArgs.now, 1_000);
    assert.equal(bindArgs.variantId, "youtube-queue_1-job_1");
    assert.deepEqual(bindArgs.platformFields, {
      privacy: "private",
      title: "Clip title",
      description: "Clip description",
      tags: ["montage", "launch"],
      thumbnailRef: "file:///tmp/thumb.jpg",
    });
    return { id: "target_1", validation_state: "pending" } as T;
  }
  if (command === "social_validate_target") {
    assert.deepEqual(args, { targetId: "target_1", now: 1_000 });
    return { id: "target_1", validation_state: "valid" } as T;
  }
  if (command === "social_schedule_target") {
    const scheduleArgs = args?.args as {
      artifactRef?: string;
      jobId?: string;
      now?: number;
    };
    assert.equal(scheduleArgs.jobId, "job_1");
    assert.equal(scheduleArgs.artifactRef, "");
    assert.equal(scheduleArgs.now, 1_000);
    return { id: "job_1", status: "scheduled" } as T;
  }
  if (command === "social_upload_artifact") {
    assert.deepEqual(args, { jobId: "job_1", filePath: "/tmp/render.mp4" });
    return { id: "job_1", status: "scheduled", scheduledFor: 1_001 } as T;
  }
  if (command === "social_fire_due_job") {
    assert.deepEqual(args, { jobId: "job_1" });
    return { id: "job_1", status: "processing" } as T;
  }
  if (command === "social_poll_publish_job") {
    assert.equal(args?.jobId, "job_1");
    return {
      id: "job_1",
      status: "published",
      providerPostId: "yt_1",
      providerPostUrl: "https://youtube.example/watch?v=yt_1",
    } as T;
  }
  throw new Error(`unexpected command: ${command}`);
}

{
  const updates: string[] = [];
  const sleeps: number[] = [];
  let now = 1_000;
  const result = await publishRenderTargetsViaServer({
    renderQueueId: "queue_1",
    renderJobId: "render_1",
    outputPath: "/tmp/render.mp4",
    title: "YouTube 1080p",
    targets: ["youtube"],
    metadataByProvider: {
      youtube: {
        title: "Clip title",
        description: "Clip description",
        tags: ["montage", "launch"],
        visibility: "private",
        scheduledAt: undefined,
        thumbnailPath: "/tmp/thumb.jpg",
      },
    },
    invoke,
    idFactory: (prefix) => `${prefix}_1`,
    nowSeconds: () => now,
    sleepMs: async (ms) => {
      sleeps.push(ms);
      now += Math.ceil(ms / 1000);
    },
    onState: (provider, state) => updates.push(`${provider}:${state.state}`),
  });

  assert.deepEqual(sleeps, [1_000, 2_000]);
  assert.deepEqual(calls, [
    "social_accounts",
    "social_bind_target",
    "social_validate_target",
    "social_schedule_target",
    "social_upload_artifact",
    "social_fire_due_job",
    "social_poll_publish_job",
  ]);
  assert.deepEqual(updates, [
    "youtube:uploading",
    "youtube:processing",
    "youtube:published",
  ]);
  assert.deepEqual(result.states.youtube, {
    state: "published",
    remote_url: "https://youtube.example/watch?v=yt_1",
    remote_id: "yt_1",
  });
  assert.deepEqual(result.publishedUrls, {
    youtube: "https://youtube.example/watch?v=yt_1",
  });
}

calls.length = 0;

{
  const updates: string[] = [];
  const sleeps: number[] = [];
  let now = 1_000;
  let fireAttempts = 0;
  async function racedFireInvoke<T>(
    command: string,
    args?: Record<string, unknown>,
  ): Promise<T> {
    calls.push(command);
    if (command === "social_accounts") {
      return [
        {
          id: "acct_yt",
          provider: "youtube",
          capabilities: { uploadVideo: true },
        },
      ] as T;
    }
    if (command === "social_bind_target") {
      return { id: "target_1" } as T;
    }
    if (command === "social_validate_target") {
      return { id: "target_1", validation_state: "valid" } as T;
    }
    if (command === "social_schedule_target") {
      return { id: "job_race", status: "scheduled", scheduledFor: 1_001 } as T;
    }
    if (command === "social_upload_artifact") {
      return { id: "job_race", status: "scheduled", scheduledFor: 1_001 } as T;
    }
    if (command === "social_fire_due_job") {
      assert.equal(args?.jobId, "job_race");
      fireAttempts += 1;
      return fireAttempts === 1
        ? ({ id: "job_race", status: "scheduled", scheduledFor: 1_001 } as T)
        : ({ id: "job_race", status: "processing" } as T);
    }
    if (command === "social_poll_publish_job") {
      assert.equal(args?.jobId, "job_race");
      return {
        id: "job_race",
        status: "published",
        providerPostId: "yt_race",
        providerPostUrl: "https://youtube.example/watch?v=yt_race",
      } as T;
    }
    if (command === "social_publish_job") {
      throw new Error("near-due scheduled jobs should keep firing, not read only");
    }
    throw new Error(`unexpected command: ${command}`);
  }

  const result = await publishRenderTargetsViaServer({
    renderQueueId: "queue_1",
    renderJobId: "render_1",
    outputPath: "/tmp/render.mp4",
    title: "YouTube 1080p",
    targets: ["youtube"],
    metadataByProvider: {},
    invoke: racedFireInvoke,
    idFactory: (prefix) => `${prefix}_1`,
    nowSeconds: () => now,
    sleepMs: async (ms) => {
      sleeps.push(ms);
      now += Math.ceil(ms / 1000);
    },
    onState: (provider, state) => updates.push(`${provider}:${state.state}`),
  });

  assert.deepEqual(calls, [
    "social_accounts",
    "social_bind_target",
    "social_validate_target",
    "social_schedule_target",
    "social_upload_artifact",
    "social_fire_due_job",
    "social_fire_due_job",
    "social_poll_publish_job",
  ]);
  assert.deepEqual(sleeps, [1_000, 2_000, 2_000]);
  assert.deepEqual(updates, [
    "youtube:uploading",
    "youtube:scheduled",
    "youtube:processing",
    "youtube:published",
  ]);
  assert.deepEqual(result.states.youtube, {
    state: "published",
    remote_url: "https://youtube.example/watch?v=yt_race",
    remote_id: "yt_race",
  });
}

calls.length = 0;

{
  async function requiresActionInvoke<T>(command: string): Promise<T> {
    calls.push(command);
    if (command === "social_accounts") {
      return [
        {
          id: "acct_yt",
          provider: "youtube",
          capabilities: { uploadVideo: true },
        },
      ] as T;
    }
    if (command === "social_bind_target") return { id: "target_1" } as T;
    if (command === "social_validate_target") {
      return { id: "target_1", validation_state: "valid" } as T;
    }
    if (command === "social_schedule_target") {
      return { id: "job_action", status: "scheduled" } as T;
    }
    if (command === "social_upload_artifact") {
      return {
        id: "job_action",
        status: "requires_action",
        requiresActionReason: "missing_scope",
      } as T;
    }
    throw new Error(`unexpected command: ${command}`);
  }

  const result = await publishRenderTargetsViaServer({
    renderQueueId: "queue_1",
    renderJobId: "render_1",
    outputPath: "/tmp/render.mp4",
    title: "YouTube 1080p",
    targets: ["youtube"],
    metadataByProvider: {},
    invoke: requiresActionInvoke,
    idFactory: (prefix) => `${prefix}_1`,
    nowSeconds: () => 1_000,
  });

  assert.deepEqual(result.states.youtube, {
    state: "requires_action",
    reason: "missing scope",
    job_id: "job_action",
  });
}

calls.length = 0;

{
  let boundAccountId: string | undefined;
  async function selectedAccountInvoke<T>(
    command: string,
    args?: Record<string, unknown>,
  ): Promise<T> {
    calls.push(command);
    if (command === "social_accounts") {
      return [
        {
          id: "acct_yt_a",
          provider: "youtube",
          capabilities: { uploadVideo: true },
        },
        {
          id: "acct_yt_b",
          provider: "youtube",
          capabilities: { uploadVideo: true },
        },
      ] as T;
    }
    if (command === "social_bind_target") {
      const bindArgs = args?.args as {
        connectedAccountId?: string;
      };
      boundAccountId = bindArgs.connectedAccountId;
      return { id: "target_1" } as T;
    }
    if (command === "social_validate_target") {
      return { id: "target_1", validationState: "valid" } as T;
    }
    if (command === "social_schedule_target") {
      return { id: "job_1", status: "scheduled" } as T;
    }
    if (command === "social_upload_artifact") {
      return { id: "job_1", status: "published" } as T;
    }
    if (command === "social_fire_due_job") {
      throw new Error("already-published jobs should not be fired");
    }
    throw new Error(`unexpected command: ${command}`);
  }

  const result = await publishRenderTargetsViaServer({
    renderQueueId: "queue_1",
    renderJobId: "render_1",
    outputPath: "/tmp/render.mp4",
    title: "YouTube 1080p",
    targets: ["youtube"],
    accountIdsByProvider: { youtube: "acct_yt_b" },
    metadataByProvider: {},
    invoke: selectedAccountInvoke,
    idFactory: (prefix) => `${prefix}_1`,
    nowSeconds: () => 1_000,
  });
  assert.equal(boundAccountId, "acct_yt_b");
  assert.notEqual(result.states.youtube.state, "failed");
}

calls.length = 0;

{
  const updates: string[] = [];
  async function privateTikTokInvoke<T>(
    command: string,
    args?: Record<string, unknown>,
  ): Promise<T> {
    calls.push(command);
    if (command === "social_accounts") {
      return [
        {
          id: "acct_tt",
          provider: "tiktok",
          capabilities: { uploadVideo: true },
        },
      ] as T;
    }
    if (command === "social_bind_target") {
      const bindArgs = args?.args as { platformFields?: Record<string, unknown> };
      assert.deepEqual(bindArgs.platformFields, {
        privacy: "private",
        title: "TikTok private clip",
        disableDuet: false,
        disableComment: false,
        disableStitch: false,
      });
      return { id: "target_1" } as T;
    }
    if (command === "social_validate_target") {
      return { id: "target_1", validation_state: "valid" } as T;
    }
    if (command === "social_schedule_target") {
      return { id: "job_private_tiktok", status: "scheduled" } as T;
    }
    if (command === "social_upload_artifact") {
      return {
        id: "job_private_tiktok",
        status: "processing",
        providerPostId: "v_pub_file~private",
      } as T;
    }
    if (command === "social_fire_due_job") {
      throw new Error("processing jobs should not be fired");
    }
    if (command === "social_poll_publish_job") {
      assert.equal(args?.jobId, "job_private_tiktok");
      return {
        id: "job_private_tiktok",
        status: "published",
        providerPostId: "v_pub_file~private",
        providerPostUrl: null,
      } as T;
    }
    throw new Error(`unexpected command: ${command}`);
  }

  const result = await publishRenderTargetsViaServer({
    renderQueueId: "queue_1",
    renderJobId: "render_1",
    outputPath: "/tmp/render.mp4",
    title: "TikTok private clip",
    targets: ["tiktok"],
    metadataByProvider: {
      tiktok: {
        title: "TikTok private clip",
        description: "Ignored unused description",
        tags: ["ignored"],
        visibility: "private",
        scheduledAt: undefined,
        thumbnailPath: "/tmp/ignored-thumb.jpg",
      },
    },
    invoke: privateTikTokInvoke,
    idFactory: (prefix) => `${prefix}_1`,
    nowSeconds: () => 1_000,
    onState: (provider, state) => updates.push(`${provider}:${state.state}`),
  });

  assert.deepEqual(updates, [
    "tiktok:uploading",
    "tiktok:processing",
    "tiktok:published",
  ]);
  assert.deepEqual(result.states.tiktok, {
    state: "published",
    remote_id: "v_pub_file~private",
  });
  assert.deepEqual(result.publishedUrls, {});
}

calls.length = 0;

{
  async function twitterXInvoke<T>(
    command: string,
    args?: Record<string, unknown>,
  ): Promise<T> {
    calls.push(command);
    if (command === "social_accounts") {
      return [
        {
          id: "acct_x",
          provider: "twitter_x",
          capabilities: { uploadVideo: true },
        },
      ] as T;
    }
    if (command === "social_bind_target") {
      const bindArgs = args?.args as { platformFields?: Record<string, unknown> };
      assert.deepEqual(bindArgs.platformFields, {
        title: "X post text",
      });
      return { id: "target_x" } as T;
    }
    if (command === "social_validate_target") {
      return { id: "target_x", validation_state: "valid" } as T;
    }
    if (command === "social_schedule_target") {
      return { id: "job_x", status: "scheduled", scheduledFor: 2_000 } as T;
    }
    if (command === "social_upload_artifact") {
      return { id: "job_x", status: "scheduled", scheduledFor: 2_000 } as T;
    }
    throw new Error(`unexpected command: ${command}`);
  }

  const result = await publishRenderTargetsViaServer({
    renderQueueId: "queue_1",
    renderJobId: "render_1",
    outputPath: "/tmp/render.mp4",
    title: "Fallback X text",
    targets: ["twitter_x"],
    metadataByProvider: {
      twitter_x: {
        title: "X post text",
        description: "Ignored X description",
        tags: ["ignored"],
        visibility: "private",
        scheduledAt: 2_000,
        thumbnailPath: "/tmp/ignored-x-thumb.jpg",
      },
    },
    invoke: twitterXInvoke,
    idFactory: (prefix) => `${prefix}_x`,
    nowSeconds: () => 1_000,
  });

  assert.deepEqual(result.states.twitter_x, {
    state: "scheduled",
    job_id: "job_x",
    scheduled_for: 2_000,
  });
}

calls.length = 0;

{
  async function futureScheduledInvoke<T>(
    command: string,
    args?: Record<string, unknown>,
  ): Promise<T> {
    calls.push(command);
    if (command === "social_accounts") {
      return [
        {
          id: "acct_yt",
          provider: "youtube",
          capabilities: { uploadVideo: true },
        },
      ] as T;
    }
    if (command === "social_bind_target") return { id: "target_1" } as T;
    if (command === "social_validate_target") {
      return { id: "target_1", validation_state: "valid" } as T;
    }
    if (command === "social_schedule_target") {
      return { id: "job_future", status: "scheduled", scheduledFor: 1_900 } as T;
    }
    if (command === "social_upload_artifact") {
      return { id: "job_future", status: "scheduled", scheduledFor: 1_900 } as T;
    }
    if (command === "social_fire_due_job") {
      throw new Error("future scheduled jobs should not fire immediately");
    }
    if (command === "social_publish_job") {
      return { id: "job_future", status: "scheduled", scheduledFor: 1_900 } as T;
    }
    throw new Error(`unexpected command: ${command}`);
  }

  const result = await publishRenderTargetsViaServer({
    renderQueueId: "queue_1",
    renderJobId: "render_1",
    outputPath: "/tmp/render.mp4",
    title: "Scheduled YouTube",
    targets: ["youtube"],
    metadataByProvider: {
      youtube: {
        title: "Scheduled YouTube",
        description: "",
        tags: [],
        visibility: "private",
        scheduledAt: 1_900,
      },
    },
    invoke: futureScheduledInvoke,
    idFactory: (prefix) => `${prefix}_1`,
    nowSeconds: () => 1_000,
  });

  assert.deepEqual(calls, [
    "social_accounts",
    "social_bind_target",
    "social_validate_target",
    "social_schedule_target",
    "social_upload_artifact",
  ]);
  assert.deepEqual(result.states.youtube, {
    state: "scheduled",
    job_id: "job_future",
    scheduled_for: 1_900,
  });
}

calls.length = 0;

{
  async function failedJobInvoke<T>(
    command: string,
    args?: Record<string, unknown>,
  ): Promise<T> {
    calls.push(command);
    if (command === "social_accounts") {
      return [
        {
          id: "acct_yt",
          provider: "youtube",
          capabilities: { uploadVideo: true },
        },
      ] as T;
    }
    if (command === "social_bind_target") return { id: "target_1" } as T;
    if (command === "social_validate_target") {
      return { id: "target_1", validation_state: "valid" } as T;
    }
    if (command === "social_schedule_target") {
      return { id: "job_failed", status: "scheduled" } as T;
    }
    if (command === "social_upload_artifact") {
      return { id: "job_failed", status: "scheduled" } as T;
    }
    if (command === "social_fire_due_job") {
      assert.equal(args?.jobId, "job_failed");
      return { id: "job_failed", status: "failed", normalizedError: "youtube_title_required" } as T;
    }
    if (command === "social_poll_publish_job") {
      assert.equal(args?.jobId, "job_failed");
      return {
        id: "job_failed",
        status: "failed",
        normalizedError: "youtube_title_required",
      } as T;
    }
    throw new Error(`unexpected command: ${command}`);
  }

  const result = await publishRenderTargetsViaServer({
    renderQueueId: "queue_1",
    renderJobId: "render_1",
    outputPath: "/tmp/render.mp4",
    title: "YouTube 1080p",
    targets: ["youtube"],
    metadataByProvider: {},
    invoke: failedJobInvoke,
    idFactory: (prefix) => `${prefix}_1`,
    nowSeconds: () => 1_000,
  });
  assert.deepEqual(result.states.youtube, {
    state: "failed",
    reason: "youtube title required",
    job_id: "job_failed",
  });
}

calls.length = 0;

{
  async function invalidTargetInvoke<T>(command: string): Promise<T> {
    calls.push(command);
    if (command === "social_accounts") {
      return [
        {
          id: "acct_yt",
          provider: "youtube",
          capabilities: { uploadVideo: true },
        },
      ] as T;
    }
    if (command === "social_bind_target") return { id: "target_1" } as T;
    if (command === "social_validate_target") {
      return {
        id: "target_1",
        validationState: "invalid",
        validationReasons: ["title.required"],
      } as T;
    }
    throw new Error(`unexpected command: ${command}`);
  }

  const result = await publishRenderTargetsViaServer({
    renderQueueId: "queue_1",
    renderJobId: "render_1",
    outputPath: "/tmp/render.mp4",
    title: "YouTube 1080p",
    targets: ["youtube"],
    metadataByProvider: {},
    invoke: invalidTargetInvoke,
    idFactory: (prefix) => `${prefix}_1`,
    nowSeconds: () => 1_000,
  });
  assert.deepEqual(result.states.youtube, {
    state: "failed",
    reason: "Not valid: title required",
  });
}

calls.length = 0;

{
  async function noAccountInvoke<T>(command: string): Promise<T> {
    calls.push(command);
    if (command === "social_accounts") return [] as T;
    throw new Error(`unexpected command: ${command}`);
  }
  const result = await publishRenderTargetsViaServer({
    renderQueueId: "queue_1",
    renderJobId: "render_1",
    outputPath: "/tmp/render.mp4",
    title: "YouTube 1080p",
    targets: ["youtube"],
    metadataByProvider: {},
    invoke: noAccountInvoke,
  });
  assert.equal(result.states.youtube.state, "failed");
  if (result.states.youtube.state === "failed") {
    assert.match(result.states.youtube.reason, /No upload-capable youtube/);
  }
  assert.deepEqual(calls, ["social_accounts"]);
}

console.log("server-publish: OK");
