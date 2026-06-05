import type { RenderUploadState } from "./renderQueue";
import type { UploadMetadata } from "../state/uploadMetadata";

type InvokeFn = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;

type AccountSummaryLite = {
  id: string;
  provider: string;
  capabilities: { uploadVideo: boolean };
};

type PublishJob = {
  id: string;
  status: string;
  providerPostId?: string | null;
  providerPostUrl?: string | null;
  normalizedError?: string | null;
  requiresActionReason?: string | null;
};

const JOB_POLL_INTERVAL_MS = 2_000;
const JOB_POLL_TIMEOUT_MS = 180_000;

type ValidatedTarget = {
  validation_state?: string;
  validationState?: string;
};

export type ServerRenderPublishInput = {
  renderQueueId: string;
  renderJobId: string;
  outputPath: string;
  title: string;
  targets: string[];
  metadataByProvider: Record<string, UploadMetadata>;
  invoke: InvokeFn;
  idFactory?: (prefix: string) => string;
  nowSeconds?: () => number;
  onState?: (provider: string, state: RenderUploadState) => void;
};

export type ServerRenderPublishResult = {
  states: Record<string, RenderUploadState>;
  publishedUrls: Record<string, string>;
};

function randomId(prefix: string): string {
  const bytes = new Uint8Array(16);
  crypto.getRandomValues(bytes);
  const hex = Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
  return `${prefix}-${hex}`;
}

function defaultNowSeconds(): number {
  return Math.floor(Date.now() / 1000);
}

function uploadCapableAccount(
  accounts: AccountSummaryLite[],
  provider: string,
): AccountSummaryLite | undefined {
  return accounts.find(
    (account) =>
      account.provider === provider && account.capabilities.uploadVideo,
  );
}

function failed(reason: string, jobId?: string): RenderUploadState {
  return jobId
    ? { state: "failed", reason, job_id: jobId }
    : { state: "failed", reason };
}

function stateFromServerJob(job: PublishJob): RenderUploadState {
  if (job.status === "published" && job.providerPostUrl) {
    return {
      state: "published",
      remote_url: job.providerPostUrl,
      remote_id: job.providerPostId ?? job.id,
    };
  }
  if (
    job.status === "failed" ||
    job.status === "requires_action" ||
    job.status === "cancelled"
  ) {
    return failed(
      job.normalizedError ??
        job.requiresActionReason ??
        `server publish ${job.status}`,
      job.id,
    );
  }
  if (job.status === "processing" || job.status === "uploading") {
    return { state: "processing", job_id: job.id };
  }
  return { state: "scheduled", job_id: job.id };
}

function validationState(target: ValidatedTarget): string | undefined {
  return target.validation_state ?? target.validationState;
}

function localArtifactRef(outputPath: string): string {
  return `file://${outputPath}`;
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function isTerminalJob(job: PublishJob): boolean {
  return (
    job.status === "published" ||
    job.status === "failed" ||
    job.status === "requires_action" ||
    job.status === "cancelled"
  );
}

async function pollServerPublishJob(
  invoke: InvokeFn,
  provider: string,
  job: PublishJob,
  onState?: (provider: string, state: RenderUploadState) => void,
): Promise<PublishJob> {
  let current = job;
  const started = Date.now();
  while (!isTerminalJob(current) && Date.now() - started < JOB_POLL_TIMEOUT_MS) {
    await sleep(JOB_POLL_INTERVAL_MS);
    current = await invoke<PublishJob>("social_publish_job", { jobId: current.id });
    onState?.(provider, stateFromServerJob(current));
  }
  return current;
}

export async function publishRenderTargetsViaServer({
  renderQueueId,
  renderJobId,
  outputPath,
  title,
  targets,
  metadataByProvider,
  invoke,
  idFactory = randomId,
  nowSeconds = defaultNowSeconds,
  onState,
}: ServerRenderPublishInput): Promise<ServerRenderPublishResult> {
  const states: Record<string, RenderUploadState> = {};
  const publishedUrls: Record<string, string> = {};
  const accounts = await invoke<AccountSummaryLite[]>("social_accounts");

  for (const provider of targets) {
    const account = uploadCapableAccount(accounts, provider);
    if (!account) {
      states[provider] = failed(`No upload-capable ${provider} account connected`);
      onState?.(provider, states[provider]);
      continue;
    }

    try {
      states[provider] = { state: "uploading", progress: 0 };
      onState?.(provider, states[provider]);

      const metadata = metadataByProvider[provider];
      const targetId = idFactory("target");
      const jobId = idFactory("job");
      const artifactRef = localArtifactRef(outputPath);
      const now = nowSeconds();
      const scheduledFor = metadata?.scheduledAt ?? now + 1;
      await invoke("social_bind_target", {
        args: {
          targetId,
          campaignId: `render-${renderJobId}`,
          variantId: `${provider}-${renderQueueId}-${jobId}`,
          connectedAccountId: account.id,
          platformFields: {
            privacy: metadata?.visibility ?? "private",
            title: metadata?.title || title,
            description: metadata?.description ?? "",
          },
          scheduledFor,
          now,
        },
      });

      const validated = await invoke<ValidatedTarget>(
        "social_validate_target",
        { targetId, now },
      );
      const validatedState = validationState(validated);
      if (validatedState !== "valid") {
        states[provider] = failed(`Not valid: ${validatedState ?? "unknown"}`);
        onState?.(provider, states[provider]);
        continue;
      }

      const scheduled = await invoke<PublishJob>("social_schedule_target", {
        args: {
          targetId,
          jobId,
          artifactRef,
          createdBy: "desktop-render-queue",
          now,
        },
      });

      states[provider] = stateFromServerJob(scheduled);
      onState?.(provider, states[provider]);
      const finalJob = await pollServerPublishJob(
        invoke,
        provider,
        scheduled,
        onState,
      );
      states[provider] = stateFromServerJob(finalJob);
      if (states[provider].state === "published") {
        publishedUrls[provider] = states[provider].remote_url;
      }
      onState?.(provider, states[provider]);
    } catch (error) {
      states[provider] = failed(error instanceof Error ? error.message : String(error));
      onState?.(provider, states[provider]);
    }
  }

  return { states, publishedUrls };
}
