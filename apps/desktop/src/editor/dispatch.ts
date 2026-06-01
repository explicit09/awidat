import { serializeEdl, type EdlOp } from "../timeline/edlBuilder.ts";

export type EditorInvoke = <T = unknown>(
  command: string,
  args?: Record<string, unknown>,
) => Promise<T>;

export type EditorDispatch = {
  proposeUserEdit: (ops: EdlOp[]) => Promise<string>;
  proposeUserEditText: (edlText: string) => Promise<string>;
  proposeVisualSupport: (selection: VisualSupportSelection) => Promise<string>;
  acceptProposal: (callId: string) => Promise<void>;
  rejectProposal: (callId: string) => Promise<void>;
};

export type VisualSupportSelection = {
  selectionText: string;
  request: string;
  anchorTranscript?: string;
  artifactType?: string;
  brollAsset?: string;
  durationS?: number;
  referenceAssets?: string[];
};

export function createEditorDispatch(invoke: EditorInvoke): EditorDispatch {
  return {
    proposeUserEdit: (ops) => proposeUserEdit(invoke, ops),
    proposeUserEditText: (edlText) => proposeUserEditText(invoke, edlText),
    proposeVisualSupport: (selection) => proposeVisualSupport(invoke, selection),
    acceptProposal: (callId) => invoke<void>("accept_proposal", { callId }),
    rejectProposal: (callId) => invoke<void>("reject_proposal", { callId }),
  };
}

async function proposeUserEdit(
  invoke: EditorInvoke,
  ops: EdlOp[],
): Promise<string> {
  if (ops.length === 0) {
    throw new Error("proposeUserEdit requires at least one EDL op");
  }
  return invoke<string>("propose_user_edit", { edlText: serializeEdl(ops) });
}

async function proposeUserEditText(
  invoke: EditorInvoke,
  edlText: string,
): Promise<string> {
  if (edlText.trim().length === 0) {
    throw new Error("proposeUserEditText requires non-empty EDL text");
  }
  return invoke<string>("propose_user_edit", { edlText });
}

async function proposeVisualSupport(
  invoke: EditorInvoke,
  selection: VisualSupportSelection,
): Promise<string> {
  if (selection.selectionText.trim().length === 0) {
    throw new Error("proposeVisualSupport requires selected transcript text");
  }
  if (selection.request.trim().length === 0) {
    throw new Error("proposeVisualSupport requires a visual-support request");
  }
  return invoke<string>("propose_visual_support", { payload: selection });
}
