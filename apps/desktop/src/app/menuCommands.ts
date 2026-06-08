export const MENU_COMMAND_EVENT = "montage-menu-command";

export const MENU_COMMANDS = {
  NEW_PROJECT: "project:new",
  OPEN_PROJECT: "project:open",
  OPEN_RECENT: "project:open_recent",
  CLOSE_PROJECT: "project:close",
  IMPORT_FILES: "asset:import_files",
  IMPORT_URL: "asset:import_url",
  RUN_INDEXERS: "project:run_indexers",
  EXPORT_TIMELINE: "timeline:export",
  REVEAL_PROJECT: "project:reveal",
  DELETE_CLIP: "edit:delete_clip",
  ACCEPT_PROPOSAL: "proposal:accept",
  REJECT_PROPOSAL: "proposal:reject",
  VIEW_MEDIA: "view:media",
  VIEW_TIMELINE: "view:timeline",
  VIEW_PROPERTIES: "view:properties",
  VIEW_NOTES: "view:notes",
  VIEW_SCOPES: "view:scopes",
  VIEW_SIDEBAR: "view:sidebar",
  VIEW_CHAT: "view:chat",
  VIEW_TRANSCRIPT: "view:transcript",
  VIEW_EDITS: "view:edits",
  TIMELINE_ZOOM_IN: "timeline:zoom_in",
  TIMELINE_ZOOM_OUT: "timeline:zoom_out",
  TIMELINE_ZOOM_FIT: "timeline:zoom_fit",
  TOGGLE_FULLSCREEN: "window:toggle_fullscreen",
  NAV_PROJECT: "nav:project",
  NAV_WORKSPACE: "nav:workspace",
  NAV_AGENT: "nav:agent",
  NAV_MEDIA: "nav:media",
  NAV_REVIEW: "nav:review",
  NAV_DELIVER: "nav:deliver",
  NAV_SETTINGS: "nav:settings",
  HELP_DOCS: "help:docs",
  HELP_SHORTCUTS: "help:shortcuts",
  HELP_LOGS: "help:logs",
  HELP_REPORT_ISSUE: "help:report_issue",
} as const;

export type MenuCommandId =
  (typeof MENU_COMMANDS)[keyof typeof MENU_COMMANDS];

export type MenuCommandEvent = CustomEvent<MenuCommandId>;

export function emitMenuCommand(id: string) {
  window.dispatchEvent(
    new CustomEvent(MENU_COMMAND_EVENT, { detail: id }),
  );
}

export function onMenuCommand(
  handler: (id: MenuCommandId) => void,
): () => void {
  const listener = (event: Event) => {
    const id = (event as CustomEvent<string>).detail;
    if (isMenuCommandId(id)) {
      handler(id);
    }
  };
  window.addEventListener(MENU_COMMAND_EVENT, listener);
  return () => window.removeEventListener(MENU_COMMAND_EVENT, listener);
}

function isMenuCommandId(id: string): id is MenuCommandId {
  return Object.values(MENU_COMMANDS).includes(id as MenuCommandId);
}
