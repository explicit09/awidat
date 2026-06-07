# Desktop UX Performance Report

Label: final
Generated: 2026-06-06T22:24:37.600Z

## Metrics

| Metric | Result | Target | Status |
| --- | ---: | ---: | --- |
| Warm startup to usable shell | 269 ms | 2500 ms | pass |
| Cold startup to usable shell | 1301 ms | 4000 ms | pass |
| Open project to first preview frame | 503 ms | 1500 ms | pass |
| Open project to interactive timeline | 291 ms | 2000 ms | pass |
| Menu/tab switch p95 | 5.9 ms | 120 ms | pass |
| Max UI long task | 0 ms | 50 ms | pass |

## Unsupported Native Metrics

- Native process start to renderer ready is not measured by this harness.
- This benchmark uses the desktop React renderer with Tauri IPC mocks, not a native WebView/WebDriver session.

## Switch Samples

Menu switch p95 is computed from 32 samples.

- Deliver #0: 3.1 ms
- Schedule #0: 4.8 ms
- Skills #0: 5.9 ms
- Stage #0: 3.7 ms
- Deliver #1: 3.1 ms
- Schedule #1: 4.7 ms
- Skills #1: 5.9 ms
- Stage #1: 3.2 ms
- Deliver #2: 2.8 ms
- Schedule #2: 4.7 ms
- Skills #2: 6.7 ms
- Stage #2: 3 ms
- Deliver #3: 2.6 ms
- Schedule #3: 4.5 ms
- Skills #3: 5.5 ms
- Stage #3: 3 ms
- Deliver #4: 2.7 ms
- Schedule #4: 4.5 ms
- Skills #4: 5.4 ms
- Stage #4: 3.1 ms
- Deliver #5: 2.7 ms
- Schedule #5: 4 ms
- Skills #5: 2.4 ms
- Stage #5: 0.9 ms
- Deliver #6: 0.6 ms
- Schedule #6: 1 ms
- Skills #6: 1.3 ms
- Stage #6: 0.7 ms
- Deliver #7: 0.7 ms
- Schedule #7: 1 ms
- Skills #7: 1.4 ms
- Stage #7: 0.7 ms

## Long Tasks

Observed 0 long task(s) over 50 ms. Max: 0 ms.

## Commands

- `npm run test:perf-full`
- `pnpm exec vite --host 127.0.0.1`
