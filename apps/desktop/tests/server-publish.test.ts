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
      tags: ["awidat", "launch"],
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
  if (command === "social_publish_job") {
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
        tags: ["awidat", "launch"],
        visibility: "private",
        scheduledAt: undefined,
        thumbnailPath: "/tmp/thumb.jpg",
      },
    },
    invoke,
    idFactory: (prefix) => `${prefix}_1`,
    nowSeconds: () => 1_000,
    onState: (provider, state) => updates.push(`${provider}:${state.state}`),
  });

  assert.deepEqual(calls, [
    "social_accounts",
    "social_bind_target",
    "social_validate_target",
    "social_schedule_target",
    "social_upload_artifact",
    "social_publish_job",
  ]);
  assert.deepEqual(updates, [
    "youtube:uploading",
    "youtube:scheduled",
    "youtube:published",
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
    if (command === "social_publish_job") {
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
