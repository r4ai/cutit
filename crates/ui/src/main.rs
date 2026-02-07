mod app;
mod bridge;
mod widgets;

use app::AppState;

fn main() -> iced::Result {
    iced::application("Cutit", AppState::update, AppState::view)
        .subscription(AppState::subscription)
        .run_with(AppState::boot)
}
