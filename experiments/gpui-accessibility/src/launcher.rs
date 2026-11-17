fn main() -> std::process::ExitCode {
    match ansa_ui_gpui::run_with_application(gpui_platform::application()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}
