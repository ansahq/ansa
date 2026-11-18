use ansa_core::{Engine, calculator};
use ansa_domain::{Command, CommandId, CommandKind};
use std::{env, process::ExitCode};

fn help_text() -> String {
    "Ansa — local-first Rust launcher\n\n  ansa search <query>    Search Windows Start Menu\n  ansa apps              List discovered applications\n  ansa calc <expression> Evaluate local arithmetic\n  ansa open <stable-id>   Explicitly launch an application\n  ansa hotkey validate <chord>  Validate a summon shortcut without registering it\n  ansa hotkey status     Probe global-summon support\n  ansa hotkey list       Show built-in bindings and the fallback\n\nDesktop: cargo run -p ansa-desktop\nNo IPC show command exists yet; use the desktop's Ctrl+Space shortcut.\nGlobal hotkeys are Windows-only in this build. When registration is unavailable, use a desktop shortcut to ansa-desktop; Ansa never simulates input or elevates to claim a shortcut."
        .to_owned()
}

fn run(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        None | Some("help" | "--help" | "-h") => {
            println!("{}", help_text());
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
        Some("hotkey") => {
            println!("{}", hotkey_command(&args[1..])?);
        }
        Some(other) => return Err(format!("Unknown command: {other}. Run `ansa --help`.")),
    }
    Ok(())
}

fn hotkey_command(args: &[String]) -> Result<String, String> {
    match args.first().map(String::as_str) {
        None | Some("help" | "--help" | "-h") => Ok(hotkey_help()),
        Some("validate") => {
            if args.len() != 2 {
                return Err("Usage: ansa hotkey validate <chord>".into());
            }
            hotkey_validate(&args[1])
        }
        Some("status") => {
            if args.len() != 1 {
                return Err("Usage: ansa hotkey status".into());
            }
            hotkey_status()
        }
        Some("list") => {
            if args.len() != 1 {
                return Err("Usage: ansa hotkey list".into());
            }
            Ok(hotkey_list())
        }
        Some("fallback") => {
            if args.len() != 1 {
                return Err("Usage: ansa hotkey fallback".into());
            }
            Ok(fallback_instructions())
        }
        Some(other) => Err(format!(
            "Unknown hotkey command: {other}. Run `ansa hotkey --help`."
        )),
    }
}

fn hotkey_help() -> String {
    "Global hotkey tools (Windows-only native registration)\n\n  ansa hotkey validate <chord>  Parse and display a chord; do not register it\n  ansa hotkey status            Probe global-summon support\n  ansa hotkey list              Show built-in bindings and fallback\n  ansa hotkey fallback          Print the no-global-shortcut fallback\n\nChord grammar: modifiers ctrl/alt/shift/meta (aliases control/option/win/\ncommand/super) followed by one key, joined with '+'. Examples: ctrl+space,\nctrl+alt+k, ctrl+shift+f12. '+' is the separator and is not a key.\n\nValidation never registers the chord. A valid chord can still conflict with\nthe IME or another application; the desktop reports that conflict separately."
        .to_owned()
}

#[cfg(windows)]
fn hotkey_validate(chord: &str) -> Result<String, String> {
    let parsed =
        ansa_platform_windows::KeyChord::parse(chord).map_err(|error| error.to_string())?;
    Ok(format!(
        "valid: {}\npersisted: {}\nnative: {}\nregistration: not attempted (validation only)",
        parsed.persisted(),
        parsed.persisted(),
        parsed.render()
    ))
}

#[cfg(not(windows))]
fn hotkey_validate(_chord: &str) -> Result<String, String> {
    Err("global hotkey validation is unsupported on this platform: no native adapter exists".into())
}

#[cfg(windows)]
fn hotkey_status() -> Result<String, String> {
    let status = ansa_platform_windows::global_hotkey_capability();
    let detail = match &status {
        ansa_platform_windows::CapabilityStatus::Available => {
            "global summon can be registered".to_owned()
        }
        ansa_platform_windows::CapabilityStatus::PermissionRequired => {
            "the host requires permission for global hotkeys".to_owned()
        }
        ansa_platform_windows::CapabilityStatus::Unsupported => {
            "no global-hotkey adapter is available".to_owned()
        }
        ansa_platform_windows::CapabilityStatus::TemporarilyUnavailable => {
            "global hotkeys are temporarily unavailable".to_owned()
        }
        ansa_platform_windows::CapabilityStatus::Degraded {
            reason,
            alternative,
        } => {
            format!("{reason}\n  alternative: {alternative}")
        }
    };
    Ok(format!("capability: {status}\n  {detail}"))
}

#[cfg(not(windows))]
fn hotkey_status() -> Result<String, String> {
    Err("global hotkeys are unsupported on this platform: no native adapter exists".into())
}

#[cfg(windows)]
fn hotkey_list() -> String {
    let default = ansa_platform_windows::KeyChord::default_summon();
    format!(
        "Built-in bindings:\n  summon  {}  {}  (development default)\n\nSupported modifiers: ctrl, alt, shift, meta (aliases: control, option, win, command, super)\nSupported keys: a-z, 0-9, f1-f24, space, enter, tab, escape, backspace, delete,\ninsert, home, end, pageup, pagedown, up, down, left, right and punctuation\n- = [ ] \\ ; ' , . / `\n\n{}\n\n{}",
        default.persisted(),
        default.render(),
        persistence_note(),
        fallback_instructions()
    )
}

#[cfg(not(windows))]
fn hotkey_list() -> String {
    format!(
        "Built-in bindings:\n  summon  ctrl+space  (development default; no native adapter on this platform)\n\n{}\n\n{}",
        persistence_note(),
        fallback_instructions()
    )
}

fn persistence_note() -> &'static str {
    "Persisted configuration is implemented by ansa-platform-api (versioned text\nformat with validation and a .bak recovery file). This CLI validates chords and\ndoes not read or write the desktop's binding yet."
}

fn fallback_instructions() -> String {
    "Fallback when global registration is unavailable:\n  - create a desktop shortcut to ansa-desktop and launch it from the taskbar or Start menu;\n  - or run `cargo run -p ansa-desktop` directly.\nAnsa does not simulate keyboard input and does not request elevation to claim a shortcut."
        .to_owned()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_text_includes_the_hotkey_surface_and_fallback() {
        let help = help_text();
        assert!(help.contains("ansa hotkey validate <chord>"));
        assert!(help.contains("never simulates input"));
        assert!(help.contains("use the desktop's Ctrl+Space shortcut"));
    }

    #[cfg(windows)]
    #[test]
    fn hotkey_validate_accepts_the_default_chord_without_registering() {
        let output = hotkey_validate("ctrl+space").unwrap();
        assert!(output.contains("valid: ctrl+space"));
        assert!(output.contains("native: Ctrl+Space"));
        assert!(output.contains("not attempted"));
    }

    #[cfg(windows)]
    #[test]
    fn hotkey_status_reports_a_real_probe() {
        let status = hotkey_status().unwrap();
        assert!(status.starts_with("capability: "));
        assert!(!status.contains("temporarily"));
    }

    #[cfg(windows)]
    #[test]
    fn hotkey_validate_rejects_unknown_and_modifier_only_chords() {
        let unknown = hotkey_validate("ctrl+hyper").unwrap_err();
        assert!(unknown.contains("hyper"));
        let modifier_only = hotkey_validate("ctrl").unwrap_err();
        assert!(modifier_only.contains("non-modifier key"));
    }

    #[test]
    fn hotkey_list_is_honest_about_persistence_and_fallback() {
        let list = hotkey_list();
        assert!(list.contains("summon"));
        assert!(list.contains("does not read or write"));
        assert!(list.contains("does not simulate keyboard input"));
        assert!(list.contains("desktop shortcut"));
    }

    #[test]
    fn hotkey_command_usage_errors_are_explicit() {
        let error = hotkey_command(&["validate".to_owned()]).unwrap_err();
        assert!(error.contains("Usage: ansa hotkey validate"));
        let error = hotkey_command(&["unknown".to_owned()]).unwrap_err();
        assert!(error.contains("Unknown hotkey command"));
        assert!(
            hotkey_command(&["fallback".to_owned()])
                .unwrap()
                .contains("desktop shortcut")
        );
    }
}
