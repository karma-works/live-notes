//! Follow the system appearance (SPEC §6.1 rule 4). One accent colour from the theme.

use gpui_kit::component::Theme;
use gpui_kit::*;

pub fn apply(cx: &mut App) {
    Theme::sync_system_appearance(None, cx);
}

/// Colour of a speaker channel in the transcript; the two must be distinguishable.
pub fn channel_color(channel: ln_types::Channel, cx: &App) -> Hsla {
    use gpui_kit::component::ActiveTheme as _;
    match channel {
        ln_types::Channel::Me => cx.theme().primary,
        ln_types::Channel::Others => cx.theme().success,
    }
}
