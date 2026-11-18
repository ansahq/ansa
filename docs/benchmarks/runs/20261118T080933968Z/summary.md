# Ansa bench run 20261118T080933968Z

- Profile: `quick`
- Profile state: run (this report; bounded quick profile, not the full or soak profile)
- Command: `ansa-bench quick --desktop=C:\Users\abhir\ansa\target\debug\ansa-desktop.exe`
- Harness duration: 20.606 s
- Machine: windows Microsoft Windows [Version 10.0.26200.9974] (x86_64) | CPU: 12th Gen Intel(R) Core(TM) i5-12450H | cores: 12 logical / 8 physical | RAM: 15.6 GiB | power: Power Scheme GUID: 27fa6203-3987-4dcc-918d-748559d549ec  (Performance) | 2026-11-18T08:09:33Z
- Build: commit aba7a38c6d692f6493f97480c2da3d98bed05650 (dirty) | rustc 1.99.1 (58cf8f9c3 2026-10-13) | profile release
- Corpus: commands ansa-commands-10k-v1 (10000 entries, sha256 c52ff8497f1052db3eed78cebd2d28ef5ae6bec75cbaa81960316d3df0256839, manifest match) | file-index ansa-file-index-1m-v1 (1000000 entries, sha256 not hashed in this run)
- Label: every number below is a **measurement**.
- Target: **budget: not defined**. No budget is met or claimed.

## Profile coverage

- `quick`: run (this report; bounded quick profile, not the full or soak profile)
- `full`: not run by this invocation
- `soak`: not run by this invocation
- `cancel-flood`: not run by this invocation
- `delayed-provider`: not run by this invocation

## Measurements

| measurement | target | unit | samples | p50 | p95 | p99 | min | max | failures | notes |
|---|---|---|---:|---:|---:|---:|---:|---:|---:|---|
| cold_launch_first_frame_internal | not defined | microseconds | 1 | 248396.0 | 248396.0 | 248396.0 | 248396.0 | 248396.0 | 0 | debug-only launcher fixture, so launch samples use target/debug/ansa-desktop.exe even when the harness is release; first spawn labelled cold without reboot/cache purge; internal marker starts at run_with_application, wall includes process creation; incomplete=0 |
| cold_launch_first_frame_wall | not defined | microseconds | 1 | 608109.0 | 608109.0 | 608109.0 | 608109.0 | 608109.0 | 0 | spawn to ANSA_PERF_FIRST_FRAME_US pipe read; includes process creation |
| warm_launch_first_frame_internal | not defined | microseconds | 2 | 223602.0 | 223677.6 | 223684.32 | 223518.0 | 223686.0 | 0 | debug-only launcher fixture, so launch samples use target/debug/ansa-desktop.exe even when the harness is release; first spawn labelled cold without reboot/cache purge; internal marker starts at run_with_application, wall includes process creation; incomplete=0 |
| warm_launch_first_frame_wall | not defined | microseconds | 2 | 554941.5 | 556605.15 | 556753.03 | 553093.0 | 556790.0 | 0 | repeated process spawn, not a hotkey summon of an already-resident launcher |
| frame_interval | not defined | microseconds | 180 | 16678.0 | 16985.5 | 17111.63 | 15729.0 | 17123.0 | 0 | delta between consecutive no-op frame callbacks; not CPU frame-work time |
| launch_failures | not defined | count | 1 | 0.0 | 0.0 | 0.0 | 0.0 | 0.0 | 0 | processes that timed out or omitted the first-frame marker |
| query_to_results | not defined | microseconds | 160 | 11945.65 | 14313.165 | 14645.461 | 7399.0 | 14855.3 | 0 | Engine::set_query + lazy results() on the frozen 10k corpus; includes ranking and result cloning, excludes UI input and paint |
| keystroke_to_list | not defined | microseconds | 340 | 14585.95 | 16027.56 | 16432.414 | 12187.0 | 17198.7 | 0 | incremental typing of 'benchmark command' against the 10k corpus; in-process model timing, not physical input latency |
| memory_working_set | not defined | bytes | 1 | 12636160.0 | 12636160.0 | 12636160.0 | 12636160.0 | 12636160.0 | 0 | process tree [ansa-bench(28476)]; Win32 working set |
| memory_rss | not defined | bytes | 1 | 12636160.0 | 12636160.0 | 12636160.0 | 12636160.0 | 12636160.0 | 0 | Windows has no separate RSS counter; this equals working set |
| memory_private_bytes | not defined | bytes | 1 | 8114176.0 | 8114176.0 | 8114176.0 | 8114176.0 | 8114176.0 | 0 | process tree [ansa-bench(28476)]; PROCESS_MEMORY_COUNTERS_EX.PrivateUsage |
| memory_pss | not defined | bytes | 1 | 7596544.0 | 7596544.0 | 7596544.0 | 7596544.0 | 7596544.0 | 0 | QueryWorkingSetEx proportional-set aggregation over [ansa-bench(28476)] |
| gpu_allocation | not defined | bytes | 0 | unavailable | unavailable | unavailable | unavailable | unavailable | 0 | no honest per-process GPU allocation counter is wired into this harness |
| process_tree_processes | not defined | count | 1 | 1.0 | 1.0 | 1.0 | 1.0 | 1.0 | 0 | ansa-bench(28476); current process plus direct children; the invoking shell parent is not attributable; named indexer/clipboard/provider workers: none by default |
| idle_cpu_time | not defined | seconds | 1 | 0.0 | 0.0 | 0.0 | 0.0 | 0.0 | 0 | ansa-bench process only; 6 GetProcessTimes samples over 5.000 s wall; no launcher worker resident |
| idle_cpu_percent_one_core | not defined | percent | 1 | 0.0 | 0.0 | 0.0 | 0.0 | 0.0 | 0 | 6 samples; percentage of one logical core, not whole-machine utilization |

## Limitations

- In-process `Engine::set_query` + `results()` timing is not end-to-end UI input latency, paint, or display latency.
- First-frame timing comes from a real debug-only launcher window. The internal marker starts at `run_with_application` entry; the wall value includes process creation. OS loader time is not included in the internal value.
- `cold_launch` labels the first spawn in this harness run. No reboot or filesystem cache purge was performed, so it is not a hardware-cold boot measurement.
- PSS uses `QueryWorkingSetEx`; GPU allocation is not measured and prints `gpu=unavailable`.
- Idle CPU is the `ansa-bench` process tree. No indexer, clipboard or provider worker is resident by default, so this is not whole-machine idle power.
- The one-million-entry file index is a synthetic harness baseline, not the production indexer.
- The `full` and `soak` profiles are not claimed unless their report says they ran to completion.
