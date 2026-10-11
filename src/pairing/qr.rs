//! QR codes for pairing links.

use qrcode::QrCode;
use qrcode::render::{svg, unicode};

/// `text` as an SVG QR code, dark modules on a light background with a quiet
/// zone so it scans from a screen in either theme.
///
/// # Errors
/// Returns a plain-language reason if `text` doesn't fit in a QR code.
pub fn svg(text: &str) -> Result<String, String> {
    let code = encode(text)?;
    Ok(code
        .render::<svg::Color<'_>>()
        .min_dimensions(240, 240)
        .quiet_zone(true)
        .dark_color(svg::Color("#000000"))
        .light_color(svg::Color("#ffffff"))
        .build())
}

/// `text` as a QR code drawn with Unicode block characters, for a terminal.
///
/// Light modules are drawn as filled blocks so it scans on a dark terminal;
/// the quiet zone is included.
///
/// # Errors
/// Returns a plain-language reason if `text` doesn't fit in a QR code.
pub fn terminal(text: &str) -> Result<String, String> {
    let code = encode(text)?;
    Ok(code
        .render::<unicode::Dense1x2>()
        .quiet_zone(true)
        .dark_color(unicode::Dense1x2::Light)
        .light_color(unicode::Dense1x2::Dark)
        .build())
}

fn encode(text: &str) -> Result<QrCode, String> {
    QrCode::new(text.as_bytes())
        .map_err(|e| format!("The pairing link is too long to fit in a QR code ({e})."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn svg_is_an_svg_document() {
        let svg = svg("https://bear.agent-residuum.com/pair#token=abc").unwrap();
        assert!(svg.contains("<svg"), "{svg}");
        assert!(svg.contains("</svg>"));
    }

    #[test]
    fn terminal_art_has_several_lines_of_blocks() {
        let art = terminal("https://bear.agent-residuum.com/pair#token=abc").unwrap();
        assert!(art.lines().count() > 10, "{art}");
    }
}
