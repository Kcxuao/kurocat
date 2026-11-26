
export type MoodMotion = 'walk' | 'grip' | 'tug' | 'stumble' | 'crouch' | 'hop' | 'land' | 'rebound' | 'stop';
type Phase = Exclude<MoodMotion, 'stop'> | 'doze' | 'wake';
type Hand = readonly [x: number, y: number];
type Body = readonly [x: number, y: number, angle: number, squash: number, tailX: number, tailY: number];
type Keyframe = readonly [ms: number, ...body: Body];

interface Clip {
  keys: readonly Keyframe[];
  left: Hand;
  right: Hand;
}

const clips: Record<Phase, Clip> = {
  walk: {
    keys: [[0, 0, 1, -0.02, 0, 6, -1]],
    left: [0.34, 0.75], right: [0.66, 0.75],
  },
  grip: {
    keys: [[0, 0, 0, 0, 0, 0, 0], [125, -5, -3, -0.12, 0, 10, 0], [260, -8, 1, -0.17, 0.045, 19, 5]],
    left: [0.28, 0.66], right: [0.48, 0.69],
  },
  tug: {
    keys: [[0, -8, 1, -0.17, 0.045, 19, 5], [125, 4, 6, 0.12, 0.05, -4, -4],
      [390, 19, 13, 0.33, 0.11, -38, 29], [530, 19, 13, 0.33, 0.11, -38, 29],
      [720, -4, -5, -0.16, -0.04, 31, -27], [820, -8, -6, -0.2, -0.03, 38, -23]],
    left: [0.23, 0.67], right: [0.44, 0.67],
  },
  stumble: {
    keys: [[0, -8, -6, -0.2, -0.03, 38, -23], [105, 26, -11, 0.29, -0.05, -43, -37],
      [225, -15, 11, -0.2, 0.14, 37, 31], [340, 12, -3, 0.11, -0.02, -23, -20],
      [460, -5, 4, -0.05, 0.06, 14, 20], [610, 0, 0, 0, 0, 0, 0]],
    left: [0.26, 0.58], right: [0.77, 0.58],
  },
  crouch: {
    keys: [[0, 0, 0, 0, 0, 0, 0], [155, -5, 22, -0.08, 0.24, -9, 29], [200, -2, 18, -0.055, 0.21, -6, 21]],
    left: [0.29, 0.77], right: [0.70, 0.77],
  },
  hop: {
    keys: [[0, -2, 18, -0.055, 0.21, -6, 21], [120, 5, -5, 0.12, -0.10, -14, -21],
      [330, 9, -12, 0.17, -0.04, -33, -35], [650, 3, -3, 0.08, 0, -12, -8]],
    left: [0.30, 0.70], right: [0.72, 0.62],
  },
  land: {
    keys: [[0, 3, -3, 0.08, 0, -12, -8], [105, 1, 20, 0.10, 0.24, 32, 31],
      [220, -4, 13, -0.11, 0.12, -21, 20], [330, 3, 4, 0.055, 0.04, 9, -8],
      [470, 0, 0, 0, 0, 0, 0]],
    left: [0.34, 0.79], right: [0.67, 0.79],
  },
  rebound: {
    keys: [[0, 0, 0, 0, 0, 0, 0], [85, 7, -30, 0.14, -0.13, -25, -40],
      [210, 8, -21, 0.19, -0.07, -34, -23], [335, -2, 12, -0.09, 0.15, 17, 23],
      [530, 0, 0, 0, 0, 0, 0]],
    left: [0.27, 0.59], right: [0.74, 0.56],
  },
  doze: {
    keys: [[0, 0, 0, 0, 0, 0, 0], [490, -3, -3, -0.09, -0.04, -7, 7], [1150, -5, 8, -0.045, 0.06, -10, 12],
      [1700, -13, 18, -0.16, 0.13, -16, 19], [1930, -6, 7, -0.07, 0.04, -12, 13],
      [2560, -15, 21, -0.20, 0.15, -23, 27], [2790, -10, 14, -0.13, 0.10, -19, 20],
      [3650, -17, 26, -0.26, 0.17, -28, 33]],
    left: [0.39, 0.59], right: [0.64, 0.77],
  },
  wake: {
    keys: [[0, -17, 26, -0.26, 0.17, -28, 33], [100, 4, -15, 0.10, -0.09, 19, -27],
      [220, -8, 1, -0.09, 0.034, -13, 11], [395, 1, 2, 0.025, 0.015, 5, 1],
      [730, 0, 0, 0, 0, 0, 0]],
    left: [0.30, 0.59], right: [0.72, 0.62],
  },
};

function sample(keys: readonly Keyframe[], ms: number): Body {
  let previous = keys[0];
  for (const next of keys.slice(1)) {
    if (ms <= next[0]) {
      const t = Math.max(0, (ms - previous[0]) / (next[0] - previous[0]));
      const eased = t * t * (3 - 2 * t);
      return [
        previous[1] + (next[1] - previous[1]) * eased,
        previous[2] + (next[2] - previous[2]) * eased,
        previous[3] + (next[3] - previous[3]) * eased,
        previous[4] + (next[4] - previous[4]) * eased,
        previous[5] + (next[5] - previous[5]) * eased,
        previous[6] + (next[6] - previous[6]) * eased,
      ];
    }
    previous = next;
  }
  return [previous[1], previous[2], previous[3], previous[4], previous[5], previous[6]];
}

export function sampleDaFeiYuMotion(phase: Phase, ms: number) {
  const clip = clips[phase];
  let [x, y, tilt, squash, tailX, tailY] = sample(clip.keys, ms);
  let { left, right } = clip;
  let face: 'idle' | 'blink' | 'shy' | 'happy' = 'idle';
  if (phase === 'walk') {
    const step = Math.sin(ms * 0.024);
    x += step * 3;
    y -= Math.abs(step) * 7;
    tailY += step * 11;
    left = [0.32, 0.75 + step * 0.068];
    right = [0.67, 0.75 - step * 0.068];
  } else if (phase === 'tug' && ms > 180 && ms < 670) {
    face = 'blink';
  } else if (phase === 'land' && ms < 165) {
    face = 'blink';
  } else if (phase === 'hop' || phase === 'rebound') {
    face = 'happy';
  } else if (phase === 'doze') {
    face = 'blink';
    if (ms > 3650) {
      const breath = Math.sin((ms - 3650) * 0.0028);
      y += breath * 3.5;
      tilt += breath * 0.018;
      tailY += breath * 4;
    }
  } else if (phase === 'wake' && ms > 230) {
    face = 'shy';
  }
  const yawn = phase === 'doze' && ms < 1000 ? Math.sin(Math.PI * ms / 1000) : 0;
  const eyesWide = (phase === 'stumble' || phase === 'wake') && ms < 230;
  const sleeping = phase === 'doze' && ms > 2810;
  return { x, y, tilt, squash, tailX, tailY, left, right, face, yawn, eyesWide, sleeping, ms };
}
