import {defineConfig} from 'vite';
import motionCanvasPlugin from '@motion-canvas/vite-plugin';

const motionCanvas = motionCanvasPlugin.default ?? motionCanvasPlugin;

export default defineConfig({
  plugins: [motionCanvas()],
});
