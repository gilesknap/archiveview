//! Fonts and colours. `install_fonts` is bobview's (`style.rs`); the trace
//! palette is a Tableau-like set in bobview's order, lightened in dark mode.

use std::sync::Arc;

use egui::{Color32, FontFamily};

/// A font family registered by [`install_fonts`]; regular is egui's
/// proportional.
pub const BOLD: &str = "bold";
const ITALIC: &str = "italic";
const BOLD_ITALIC: &str = "bold-italic";

/// Install Liberation Sans (the font Phoebus and bobview use) ahead of
/// egui's built-in fonts, which stay as fallbacks for symbols. Only the
/// standalone app calls this: an embedded plot uses its host's fonts.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let mut add = |key: &str, data: &'static [u8]| {
        fonts
            .font_data
            .insert(key.into(), Arc::new(egui::FontData::from_static(data)));
        key.to_string()
    };
    let regular = add(
        "liberation",
        include_bytes!("../fonts/LiberationSans-Regular.ttf"),
    );
    let bold = add(
        "liberation-bold",
        include_bytes!("../fonts/LiberationSans-Bold.ttf"),
    );
    let italic = add(
        "liberation-italic",
        include_bytes!("../fonts/LiberationSans-Italic.ttf"),
    );
    let bold_italic = add(
        "liberation-bold-italic",
        include_bytes!("../fonts/LiberationSans-BoldItalic.ttf"),
    );
    let symbols = add(
        "noto-math-symbols",
        include_bytes!("../fonts/NotoSansMath-Symbols.ttf"),
    );

    let mut fallback = fonts.families[&FontFamily::Proportional].clone();
    fallback.push(symbols.clone());
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .push(symbols);
    let with = |first: String| {
        std::iter::once(first)
            .chain(fallback.iter().cloned())
            .collect::<Vec<_>>()
    };
    fonts
        .families
        .insert(FontFamily::Name(BOLD.into()), with(bold));
    fonts
        .families
        .insert(FontFamily::Name(ITALIC.into()), with(italic));
    fonts
        .families
        .insert(FontFamily::Name(BOLD_ITALIC.into()), with(bold_italic));
    fonts
        .families
        .insert(FontFamily::Proportional, with(regular));
    ctx.set_fonts(fonts);
}

/// A bold font of `size`, when the app installed one; else egui's default
/// (an embedding host may not have [`BOLD`]).
pub fn bold(ctx: &egui::Context, size: f32) -> egui::FontId {
    let family = FontFamily::Name(BOLD.into());
    let has = ctx.fonts(|f| f.families().contains(&family));
    egui::FontId::new(
        size,
        if has {
            family
        } else {
            FontFamily::Proportional
        },
    )
}

/// Trace colours, in the order PVs are added: bobview's sequence (blue,
/// red, green, orange, purple, teal, brown, pink, grey, olive) in the
/// Tableau 10 shades, which read well on white and on dark grey.
const PALETTE: &[[u8; 3]] = &[
    [31, 119, 180],
    [214, 39, 40],
    [44, 160, 44],
    [255, 127, 14],
    [148, 103, 189],
    [23, 190, 207],
    [140, 86, 75],
    [227, 119, 194],
    [127, 127, 127],
    [188, 189, 34],
];

/// The `i`th palette colour, for a light or dark background.
pub fn palette(i: usize, dark: bool) -> Color32 {
    let [r, g, b] = PALETTE[i % PALETTE.len()];
    adapt(Color32::from_rgb(r, g, b), dark)
}

/// A colour as drawn on a dark background: moved 22% of the way to white
/// so it keeps its contrast.
pub fn adapt(c: Color32, dark: bool) -> Color32 {
    if !dark {
        return c;
    }
    let up = |v: u8| v + ((255 - v) as f32 * 0.22) as u8;
    Color32::from_rgb(up(c.r()), up(c.g()), up(c.b()))
}

/// `#rrggbb` (or `rrggbb`) as a colour.
pub fn parse_hex(text: &str) -> Option<Color32> {
    let h = text.trim().trim_start_matches('#');
    if h.len() != 6 || !h.is_ascii() {
        return None;
    }
    let n = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some(Color32::from_rgb(n(0)?, n(2)?, n(4)?))
}

/// The alarm colours of the Phoebus palette (MINOR, MAJOR, INVALID).
pub fn severity(sev: pv::Severity) -> Option<Color32> {
    match sev {
        pv::Severity::NoAlarm => None,
        pv::Severity::Minor => Some(Color32::from_rgb(255, 128, 0)),
        pv::Severity::Major => Some(Color32::from_rgb(255, 0, 0)),
        pv::Severity::Invalid => Some(Color32::from_rgb(255, 0, 255)),
    }
}

/// The "live" accent: the Live pill's dot (only; the now marker is grey, as
/// red is also a trace and an alarm colour).
pub fn live(dark: bool) -> Color32 {
    if dark {
        Color32::from_rgb(255, 99, 99)
    } else {
        Color32::from_rgb(220, 38, 38)
    }
}

/// The archiver pill's dot when the archiver answers.
pub fn reachable(dark: bool) -> Color32 {
    if dark {
        Color32::from_rgb(74, 222, 128)
    } else {
        Color32::from_rgb(22, 163, 74)
    }
}

/// The archiver pill's dot when the archiver fails: amber, so it is not
/// taken for the Live pill's red dot beside it.
pub fn unreachable(dark: bool) -> Color32 {
    if dark {
        Color32::from_rgb(251, 191, 36)
    } else {
        Color32::from_rgb(217, 119, 6)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_parse_and_cycle() {
        assert_eq!(parse_hex("#d62728"), Some(Color32::from_rgb(214, 39, 40)));
        assert_eq!(parse_hex("1f77b4"), Some(Color32::from_rgb(31, 119, 180)));
        assert_eq!(parse_hex("#12345"), None);
        assert_eq!(parse_hex("#gg0000"), None);
        assert_eq!(palette(0, false), palette(10, false));
        let (l, d) = (palette(0, false), palette(0, true));
        assert!(
            d.r() > l.r() && d.g() > l.g() && d.b() > l.b(),
            "lighter on dark"
        );
    }
}
