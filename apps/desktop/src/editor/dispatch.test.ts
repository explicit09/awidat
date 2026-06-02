import { strict as assert } from "node:assert";
import { createEditorDispatch, type EditorInvoke } from "./dispatch.ts";

type RecordedCall = {
  command: string;
  args: Record<string, unknown> | undefined;
};

function recorder(result = "ok") {
  const calls: RecordedCall[] = [];
  const invoke: EditorInvoke = async (command, args) => {
    calls.push({ command, args });
    return result;
  };
  return { calls, invoke };
}

function describe(name: string, fn: () => void) {
  console.log(`\n# ${name}`);
  fn();
}

function it(name: string, fn: () => Promise<void> | void) {
  Promise.resolve()
    .then(fn)
    .then(() => console.log(`  ok  ${name}`))
    .catch((err: unknown) => {
      console.error(`  FAIL  ${name}`);
      console.error("    " + (err instanceof Error ? err.message : String(err)));
      process.exitCode = 1;
    });
}

describe("createEditorDispatch", () => {
  it("serializes EDL ops through the user proposal command", async () => {
    const { calls, invoke } = recorder("user-edit-1");
    const dispatch = createEditorDispatch(invoke);

    const id = await dispatch.proposeUserEdit([
      {
        kind: "trim_clip",
        anchor: { kind: "clip_uuid", uuid: "clip-1" },
        start: 1,
        end: 4.25,
      },
    ]);

    assert.equal(id, "user-edit-1");
    assert.deepEqual(calls, [
      {
        command: "propose_user_edit",
        args: {
          edlText:
            "*** Begin EDL\n*** Trim Clip\n@@ anchor: clip_uuid=clip-1\n+ start: 1.000\n+ end: 4.250\n*** End EDL\n",
        },
      },
    ]);
  });

  it("routes proposal decisions through one command facade", async () => {
    const { calls, invoke } = recorder();
    const dispatch = createEditorDispatch(invoke);

    await dispatch.acceptProposal("call-1");
    await dispatch.rejectProposal("call-2");

    assert.deepEqual(calls, [
      { command: "accept_proposal", args: { callId: "call-1" } },
      { command: "reject_proposal", args: { callId: "call-2" } },
    ]);
  });

  it("can submit an already serialized EDL envelope", async () => {
    const { calls, invoke } = recorder("user-edit-raw");
    const dispatch = createEditorDispatch(invoke);

    const id = await dispatch.proposeUserEditText("*** Begin EDL\n*** End EDL\n");

    assert.equal(id, "user-edit-raw");
    assert.deepEqual(calls, [
      {
        command: "propose_user_edit",
        args: { edlText: "*** Begin EDL\n*** End EDL\n" },
      },
    ]);
  });

  it("routes transcript visual-support selections through the proposal command", async () => {
    const { calls, invoke } = recorder("visual-support-1");
    const dispatch = createEditorDispatch(invoke);

    const id = await dispatch.proposeVisualSupport({
      selectionText: "This quote should land harder.",
      request: "make this quote pop visually",
      anchorTranscript: "quote should land harder",
      artifactType: "quote_highlight",
    });

    assert.equal(id, "visual-support-1");
    assert.deepEqual(calls, [
      {
        command: "propose_visual_support",
        args: {
          payload: {
            selectionText: "This quote should land harder.",
            request: "make this quote pop visually",
            anchorTranscript: "quote should land harder",
            artifactType: "quote_highlight",
          },
        },
      },
    ]);
  });

  it("rejects empty user edit envelopes before invoking Tauri", async () => {
    const { calls, invoke } = recorder();
    const dispatch = createEditorDispatch(invoke);

    await assert.rejects(
      () => dispatch.proposeUserEdit([]),
      /proposeUserEdit requires at least one EDL op/,
    );
    assert.deepEqual(calls, []);
  });

  it("rejects blank serialized envelopes before invoking Tauri", async () => {
    const { calls, invoke } = recorder();
    const dispatch = createEditorDispatch(invoke);

    await assert.rejects(
      () => dispatch.proposeUserEditText("  \n"),
      /proposeUserEditText requires non-empty EDL text/,
    );
    assert.deepEqual(calls, []);
  });
});
