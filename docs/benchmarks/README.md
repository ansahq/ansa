# Synthetic search traces

Generate a JSON-lines trace using the existing, fixed `project-terminal-v1` corpus:

```powershell
cargo run --release --locked -p ansa-core --example search_bench -- --jsonl > docs/benchmarks/search-local.jsonl
```

Each run includes metadata, 500 individual query/rank/result-cloning timings and a percentile summary, after 50 warmup queries. Query-case indices map to the example's five fixed synthetic strings, not user searches. Timing stops before writing each sample; output overhead can still affect subsequent samples. Record machine/OS/power/toolchain alongside the trace before comparing runs.

These artifacts are not end-to-end input/display latency, frame work, summon latency, filesystem search, idle power or process-tree memory measurements. They do not close the framework/performance gate. Checked-in traces are historical observations, not hardware-independent guarantees or CI regression thresholds.
