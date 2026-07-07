export type StageClock = {
  now(): number;
  isPlaying(): boolean;
  rate(): number;
};

export function frozenClock(t: number): StageClock {
  return {
    now() {
      return t;
    },
    isPlaying() {
      return false;
    },
    rate() {
      return 0;
    },
  };
}

export function livePreviewClock(src: {
  now(): number;
  isPlaying(): boolean;
  rate(): number;
}): StageClock {
  return {
    now() {
      return src.now();
    },
    isPlaying() {
      return src.isPlaying();
    },
    rate() {
      return src.rate();
    },
  };
}
