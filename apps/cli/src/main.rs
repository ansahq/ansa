use ansa_core::{Engine, calculator};
use ansa_domain::{Command, CommandId, CommandKind};
use std::{env, process::ExitCode};

fn run(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        None | Some("help" | "--help" | "-h") => {
            println!(
                "Ansa — local-first Rust launcher\n\n  ansa search <query>    Search Windows Start Menu\n  ansa apps              List discovered applications\n  ansa calc <expression> Evaluate local arithmetic\n  ansa open <stable-id>   Explicitly launch an application\n\nDesktop: cargo run -p ansa-desktop\nNo IPC show command exists yet; use the desktop's Ctrl+Space shortcut."
            );
        }
        Some("calc") => {
            let expression = args[1..].join(" ");
            let value = calculator::evaluate(&expression).map_err(|error| error.to_string())?;
            println!("{value}");
        }
        Some("search" | "apps" | "open") => {
            let apps = ansa_platform_windows::discover_applications()?;
            if args[0] == "open" {
                if args.len() != 2 {
                    return Err("Usage: ansa open <stable-id>".into());
                }
                let app = apps
                    .iter()
                    .find(|app| app.id == args[1])
                    .ok_or("Application not found; refresh the catalog with `ansa apps`")?;
                ansa_platform_windows::open_application(&app.path)?;
                println!("Open requested: {}", app.title);
            } else {
                let commands = apps
                    .into_iter()
                    .map(|app| {
                        Ok(Command {
                            id: CommandId::new(&app.id)
                                .map_err(|error| format!("Invalid app identity: {error:?}"))?,
                            title: app.title,
                            subtitle: Some(app.path.clone()),
                            keywords: vec![],
                            alias: None,
                            kind: CommandKind::Application { path: app.path },
                        })
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                let mut engine = Engine::new(commands);
                engine.set_query(&args[1..].join(" "));
                for command in engine.results().iter().take(100) {
                    println!(
                        "{}\t{}\t{}",
                        command.id.as_str(),
                        command.title,
                        command.subtitle.as_deref().unwrap_or_default()
                    );
                }
                println!(
                    "{} matches (display limited to 100)",
                    engine.results().len()
                );
            }
        }
        Some(other) => return Err(format!("Unknown command: {other}. Run `ansa --help`.")),
    }
    Ok(())
}

fn main() -> ExitCode {
    match run(&env::args().skip(1).collect::<Vec<_>>()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Ansa: {error}");
            ExitCode::FAILURE
        }
    }
}
