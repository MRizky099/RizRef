//! The window icon from the embedded `.ico` (see `platform::icon`).

use winit::window::Icon;

/// The application icon, the same asset `build.rs` embeds on Windows.
const APP_ICO: &[u8] = include_bytes!("../../../assets/rizref.ico");

/// `(small, big)`: winit on X11 takes one icon (`_NET_WM_ICON`), so both
/// slots carry the 64 px image; `None` if the asset cannot be decoded.
pub fn app_icons() -> (Option<Icon>, Option<Icon>) {
    let icon = icon_of_side(64);
    (icon.clone(), icon)
}

/// The icon image closest to `side` pixels (at least that big when one is).
fn icon_of_side(side: u32) -> Option<Icon> {
    let mut entries = rizref_thumb::ico_pngs(APP_ICO);
    entries.sort_by_key(|(s, _)| *s);
    let (_, png) = entries
        .iter()
        .find(|(s, _)| *s >= side)
        .or(entries.last())?;
    let image = rizref_codecs::decode(png).ok()?;
    Icon::from_rgba(image.pixels().to_vec(), image.width(), image.height()).ok()
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_embedded_icon_decodes() {
        assert!(super::icon_of_side(64).is_some());
    }
}
