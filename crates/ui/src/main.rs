mod app;
mod bridge;
mod widgets;

use app::AppState;

fn main() -> iced::Result {
    init_tracing();

    iced::application("Cutit", AppState::update, AppState::view)
        .subscription(AppState::subscription)
        .run_with(AppState::boot)
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt::try_init();
}
