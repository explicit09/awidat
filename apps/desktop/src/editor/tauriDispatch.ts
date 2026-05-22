import { invoke } from "@tauri-apps/api/core";
import { createEditorDispatch } from "./dispatch";

export const editorDispatch = createEditorDispatch(invoke);
