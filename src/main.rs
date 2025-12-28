// SPDX-License-Identifier: MPL-2.0

mod app;
mod config;
mod helper;
mod i18n;
mod update;

fn main() -> cosmic::iced::Result {
    if std::env::args().any(|arg| arg == "--helper") {
        if let Err(err) = helper::run() {
            eprintln!("privileged helper failed: {err}");
            std::process::exit(1);
        }
        return Ok(());
    }

    // Get the system's preferred languages.
    let requested_languages = i18n_embed::DesktopLanguageRequester::requested_languages();

    // Enable localizations to be applied.
    i18n::init(&requested_languages);

    // Settings for configuring the application window and iced runtime.
    let settings = cosmic::app::Settings::default().size_limits(
        cosmic::iced::Limits::NONE
            .min_width(550.0)
            .min_height(250.0),
    );

    // Starts the application's event loop with `()` as the application's flags.
    cosmic::app::run::<app::AppModel>(settings, ())
}
