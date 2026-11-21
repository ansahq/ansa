//! Prints real-machine catalog counts without launching anything.
//!
//! Run with:
//!
//! ```text
//! cargo run -p ansa-platform-windows --example catalog --offline
//! ```

fn main() {
    match ansa_platform_windows::discover_applications() {
        Ok(entries) => {
            let packaged = entries.iter().filter(|entry| entry.packaged).count();
            let broken = entries.iter().filter(|entry| entry.is_broken()).count();
            let classic = entries.len() - packaged;
            let mut titles = std::collections::HashMap::new();
            for entry in &entries {
                *titles.entry(entry.title.to_lowercase()).or_insert(0_usize) += 1;
            }
            let duplicate_titles = titles.values().filter(|count| **count > 1).count();

            let with_arguments = entries
                .iter()
                .filter(|entry| entry.arguments.is_some())
                .count();
            let mut arguments_by_target: std::collections::BTreeMap<
                String,
                std::collections::BTreeSet<String>,
            > = std::collections::BTreeMap::new();
            for entry in &entries {
                if let Some(target) = &entry.target {
                    arguments_by_target
                        .entry(target.to_lowercase())
                        .or_default()
                        .insert(entry.arguments.clone().unwrap_or_default());
                }
            }
            let argument_distinct_targets = arguments_by_target
                .values()
                .filter(|arguments| arguments.len() > 1)
                .count();

            println!("total={}", entries.len());
            println!("classic={classic}");
            println!("packaged={packaged}");
            println!("broken={broken}");
            println!("duplicate_titles={duplicate_titles}");
            println!("with_arguments={with_arguments}");
            println!("argument_distinct_targets={argument_distinct_targets}");
            println!("watcher_wired=false");

            for entry in entries.iter().take(10) {
                println!(
                    "sample packaged={} broken={} title={:?} path={:?} target={:?} args={:?}",
                    entry.packaged,
                    entry.is_broken(),
                    entry.title,
                    entry.path,
                    entry.target,
                    entry.arguments
                );
            }

            for (target, arguments) in arguments_by_target
                .iter()
                .filter(|(_, arguments)| arguments.len() > 1)
            {
                println!("argument_variant target={target:?} arguments={arguments:?}");
            }

            for entry in entries.iter().filter(|entry| entry.is_broken()) {
                println!(
                    "broken_entry title={:?} path={:?} reason={:?}",
                    entry.title,
                    entry.path,
                    entry.broken.as_deref().unwrap_or_default()
                );
            }

            for (title, count) in titles.iter().filter(|(_, count)| **count > 1) {
                println!("duplicate_title={title:?} count={count}");
                for entry in entries
                    .iter()
                    .filter(|entry| entry.title.to_lowercase() == *title)
                {
                    println!(
                        "  duplicate packaged={} path={:?}",
                        entry.packaged, entry.path
                    );
                }
            }
        }
        Err(error) => {
            eprintln!("discovery failed: {error}");
            std::process::exit(1);
        }
    }

    let capabilities = ansa_platform_windows::host_capabilities();
    match &capabilities.packaged_applications {
        ansa_platform_windows::CapabilityStatus::Degraded {
            reason,
            alternative,
        } => println!(
            "packaged_capability=degraded reason={reason:?} alternative={alternative:?} catalog={}",
            capabilities.application_catalog
        ),
        status => println!(
            "packaged_capability={status} catalog={}",
            capabilities.application_catalog
        ),
    }
}
