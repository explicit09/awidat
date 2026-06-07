# Desktop UX Performance Report

Label: current
Generated: 2026-06-06T22:12:31.790Z

## Metrics

| Metric | Result | Target | Status |
| --- | ---: | ---: | --- |
| Warm startup to usable shell | 280 ms | 2500 ms | pass |
| Cold startup to usable shell | 1316 ms | 4000 ms | pass |
| Open project to first preview frame | 550 ms | 1500 ms | pass |
| Open project to interactive timeline | 311 ms | 2000 ms | pass |
| Menu/tab switch p95 | 385.8 ms | 120 ms | fail |
| Max UI long task | 59 ms | 50 ms | fail |

## Unsupported Native Metrics

- Native process start to renderer ready is not measured by this harness.
- This benchmark uses the desktop React renderer with Tauri IPC mocks, not a native WebView/WebDriver session.

## Switch Samples

Menu switch p95 is computed from 32 samples.

## Long Tasks

Observed 1 long task(s) over 50 ms. Max: 59 ms.

## Commands

- `npm run test:perf-full`
- `pnpm exec vite --host 127.0.0.1`
