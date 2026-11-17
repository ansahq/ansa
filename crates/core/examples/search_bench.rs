//! A reproducible synthetic query microbenchmark, not an end-to-end UI budget.
//! Run: cargo run --release -p ansa-core --example search_bench
use ansa_core::Engine;
use ansa_domain::{BuiltinCommand, Command, CommandId, CommandKind};
use std::{hint::black_box, time::Instant};

fn main() {
    let jsonl = std::env::args().any(|arg| arg == "--jsonl");
    if jsonl {
        println!(
            "{{\"kind\":\"metadata\",\"corpus\":\"project-terminal-v1\",\"commands\":10000,\"samples\":500,\"warmup\":50,\"os\":\"{}\",\"arch\":\"{}\",\"debug_assertions\":{},\"unit\":\"microseconds\"}}",
            std::env::consts::OS,
            std::env::consts::ARCH,
            cfg!(debug_assertions)
        );
    }
    let commands = (0..10_000)
        .map(|i| {
            Command::new(
                CommandId::new(format!("fixture:command-{i:05}")).unwrap(),
                format!("Project {i:05} Terminal"),
                CommandKind::Builtin(BuiltinCommand::About),
            )
        })
        .collect();
    let mut engine = Engine::new(commands);
    let queries = [
        "terminal",
        "Project 0042",
        "pj042",
        "missing-xyz",
        "Project 09999 Terminal",
    ];
    let mut times = Vec::new();
    for round in 0..110 {
        for (case, query) in queries.iter().enumerate() {
            let start = Instant::now();
            engine.set_query(query);
            black_box(engine.results());
            if round >= 10 {
                let micros = start.elapsed().as_micros();
                times.push(micros);
                if jsonl {
                    println!(
                        "{{\"kind\":\"sample\",\"iteration\":{},\"query_case\":{},\"micros\":{micros}}}",
                        round - 10,
                        case
                    );
                }
            }
        }
    }
    times.sort_unstable();
    if jsonl {
        println!(
            "{{\"kind\":\"summary\",\"p50_micros\":{},\"p95_micros\":{},\"p99_micros\":{}}}",
            times[times.len() / 2],
            times[times.len() * 95 / 100],
            times[times.len() * 99 / 100]
        );
        return;
    }
    println!("Synthetic corpus: 10,000 commands; 500 measured queries, 50 warmup queries");
    println!(
        "query + ranking + result cloning (microseconds): p50={} p95={} p99={}",
        times[times.len() / 2],
        times[times.len() * 95 / 100],
        times[times.len() * 99 / 100]
    );
    println!(
        "Not physical input latency, frame time, file search, or a hardware-qualified acceptance result."
    );
}
