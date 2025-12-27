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
    let settings = cosmic::app::Settings::default()
        .size((700.0, 640.0).into())
        .size_limits(
            cosmic::iced::Limits::NONE
                .min_width(360.0)
                .min_height(180.0),
        );

    // Starts the application's event loop with `()` as the application's flags.
    cosmic::app::run::<app::AppModel>(settings, ())
}
