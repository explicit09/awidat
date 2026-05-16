export const MOVE_TIME_EPSILON_S = 0.01;

export type MoveDraft = {
  toPosition: number;
  fromPosition: number;
  atS: number;
  fromAtS: number;
};

export function shouldKeepMoveDraft(
  draft: MoveDraft,
  epsilonS = MOVE_TIME_EPSILON_S,
): boolean {
  return (
    draft.toPosition !== draft.fromPosition ||
    Math.abs(draft.atS - draft.fromAtS) >= epsilonS
  );
}
