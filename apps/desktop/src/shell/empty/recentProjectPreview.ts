import { invoke } from "@tauri-apps/api/core";

export type RecentProjectPreview = {
  src: string;
};

type CommandInvoker = (
  command: string,
  args: Record<string, string>,
) => Promise<unknown>;

const invokeCommand: CommandInvoker = (command, args) => invoke(command, args);

export async function loadRecentProjectPreview(
  projectPath: string,
  run: CommandInvoker = invokeCommand,
): Promise<RecentProjectPreview | null> {
  const mediaPath = await run("project_thumbnail", { path: projectPath });
  if (typeof mediaPath !== "string" || mediaPath.length === 0) return null;
  const src = await run("project_preview_url", {
    projectPath,
    mediaPath,
  });
  return typeof src === "string" && src.length > 0 ? { src } : null;
}
