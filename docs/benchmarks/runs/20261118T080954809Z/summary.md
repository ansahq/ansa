# Ansa bench run 20261118T080954809Z

- Profile: `delayed-provider`
- Profile state: run (this report)
- Command: `ansa-bench delayed-provider`
- Harness duration: 1.094 s
- Machine: windows Microsoft Windows [Version 10.0.26200.9974] (x86_64) | CPU: 12th Gen Intel(R) Core(TM) i5-12450H | cores: 12 logical / 8 physical | RAM: 15.6 GiB | power: Power Scheme GUID: 27fa6203-3987-4dcc-918d-748559d549ec  (Performance) | 2026-11-18T08:09:54Z
- Build: commit aba7a38c6d692f6493f97480c2da3d98bed05650 (dirty) | rustc 1.99.1 (58cf8f9c3 2026-10-13) | profile release
- Corpus: commands ansa-commands-10k-v1 (10000 entries, sha256 c52ff8497f1052db3eed78cebd2d28ef5ae6bec75cbaa81960316d3df0256839, manifest match) | file-index ansa-file-index-1m-v1 (1000000 entries, sha256 not hashed in this run)
- Label: every number below is a **measurement**.
- Target: **budget: not defined**. No budget is met or claimed.

## Profile coverage

- `quick`: not run by this invocation
- `full`: not run by this invocation
- `soak`: not run by this invocation
- `cancel-flood`: not run by this invocation
- `delayed-provider`: run (this report)

## Measurements

| measurement | target | unit | samples | p50 | p95 | p99 | min | max | failures | notes |
|---|---|---|---:|---:|---:|---:|---:|---:|---:|---|
| delayed_provider_observed_delay | not defined | milliseconds | 5 | 100.3 | 450.547 | 490.571 | 5.385 | 500.577 | 0 | simulated sleeping provider, no network; measured from spawn to thread completion, then the stale batch was rejected |
| delayed_provider_rejections | not defined | count | 1 | 5.0 | 5.0 | 5.0 | 5.0 | 5.0 | 0 | all delayed batches were rejected after query/invocation changes |

## Limitations

- In-process `Engine::set_query` + `results()` timing is not end-to-end UI input latency, paint, or display latency.
- First-frame timing comes from a real debug-only launcher window. The internal marker starts at `run_with_application` entry; the wall value includes process creation. OS loader time is not included in the internal value.
- `cold_launch` labels the first spawn in this harness run. No reboot or filesystem cache purge was performed, so it is not a hardware-cold boot measurement.
- PSS uses `QueryWorkingSetEx`; GPU allocation is not measured and prints `gpu=unavailable`.
- Idle CPU is the `ansa-bench` process tree. No indexer, clipboard or provider worker is resident by default, so this is not whole-machine idle power.
- The one-million-entry file index is a synthetic harness baseline, not the production indexer.
- The `full` and `soak` profiles are not claimed unless their report says they ran to completion.
