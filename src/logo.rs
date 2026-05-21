use std::io::{self, IsTerminal};

/// Project logo embedded at compile time from [assets/mjolnix.png](../assets/mjolnix.png).
const LOGO_PNG: &[u8] = include_bytes!("../assets/mjolnix.png");

/// Display width in terminal cells (height follows image aspect ratio).
const LOGO_WIDTH_CELLS: u32 = 28;

/// Display the logo using the terminal graphics protocol (Kitty, iTerm, Sixel, etc.).
///
/// Requires a real TTY on stdout (normal `ssh host` session). Silently skips when not
/// supported or when `MJOLNIX_NO_LOGO` is set.
pub fn show_welcome_logo() {
    if !io::stdout().is_terminal() || std::env::var("MJOLNIX_NO_LOGO").is_ok() {
        return;
    }

    let Ok(img) = image::load_from_memory(LOGO_PNG) else {
        return;
    };

    let conf = viuer::Config {
        // Draw at the current cursor, not the top-left corner of the terminal.
        absolute_offset: false,
        // Leave cursor below the image so welcome text follows immediately (no blank gap).
        restore_cursor: false,
        width: Some(LOGO_WIDTH_CELLS),
        // Let viuer derive height from aspect ratio (terminal cells are ~2:1 w:h).
        height: None,
        ..Default::default()
    };

    let _ = viuer::print(&img, &conf);
}
