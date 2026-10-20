//! System tray module for CodexBar
//!
//! Provides icon types and tray management for the Windows system tray

pub mod icon;
pub mod render;

pub use icon::LoadingPattern;
pub use render::{
    TRAY_ICON_SIZE, TrayPaceColor, UNKNOWN_PERCENT, render_bar_icon_rgba,
    render_bar_icon_rgba_tinted, render_bar_icon_rgba_with_pace,
    render_estimated_percent_icon_rgba_tinted, render_percent_icon_rgba,
    render_percent_icon_rgba_tinted, render_percent_icon_rgba_with_pace,
    render_stacked_bar_icon_rgba, render_stacked_bar_icon_rgba_tinted,
    render_stacked_bars_icon_rgba_tinted, render_stacked_bars_icon_rgba_with_pace,
};
