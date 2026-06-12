#!/usr/bin/env node
import {spawnSync} from 'node:child_process';
import {mkdirSync} from 'node:fs';
import {dirname} from 'node:path';

const args = new Map();
for (let i = 2; i < process.argv.length; i += 2) {
  args.set(process.argv[i], process.argv[i + 1]);
}

const frames = args.get('--frames') ?? 'output/frame_%05d.png';
const fps = args.get('--fps') ?? '30';
const out = args.get('--out') ?? 'generated/drawn/motion-canvas.mov';

mkdirSync(dirname(out), {recursive: true});

const ffmpegArgs = [
  '-y',
  '-framerate',
  fps,
  '-i',
  frames,
  '-c:v',
  'prores_ks',
  '-profile:v',
  '4444',
  '-pix_fmt',
  'yuva444p10le',
  out,
];

const result = spawnSync('ffmpeg', ffmpegArgs, {stdio: 'inherit'});
if (result.status !== 0) {
  process.exit(result.status ?? 1);
}

console.log(`wrote ${out}`);
