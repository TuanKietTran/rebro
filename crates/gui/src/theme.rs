//! Platform look + light/dark palettes. Values come from the Penpot design (file "rebro",
//! pages Windows / macOS / Linux (GNOME)).

use gpui_kit::{rgb, Hsla};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Platform {
    Windows,
    Mac,
    Gnome,
}

impl Platform {
    /// The OS look, overridable with REBRO_STYLE=windows|mac|gnome (to preview the others).
    pub fn current() -> Platform {
        match std::env::var("REBRO_STYLE").unwrap_or_default().to_ascii_lowercase().as_str() {
            "windows" | "win" | "fluent" => Platform::Windows,
            "mac" | "macos" => Platform::Mac,
            "gnome" | "linux" | "adwaita" => Platform::Gnome,
            _ if cfg!(target_os = "macos") => Platform::Mac,
            _ if cfg!(target_os = "windows") => Platform::Windows,
            _ => Platform::Gnome,
        }
    }
}

pub fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

/// Every colour and metric the views use.
#[derive(Clone, Copy)]
pub struct Pal {
    pub platform: Platform,
    pub dark: bool,
    pub font: &'static str,
    pub mono: &'static str,
    pub text_size: f32,
    pub small_size: f32,
    // surfaces
    pub bg: Hsla,       // window / nav pane
    pub layer: Hsla,    // content area (Windows), main column
    pub side: Hsla,     // sidebar (macOS / GNOME)
    pub card: Hsla,     // cards, grouped/boxed lists
    pub stroke: Hsla,   // card outlines
    pub sep: Hsla,      // hairline separators
    pub nav_sel: Hsla,  // selected navigation item
    pub hover: Hsla,
    pub control: Hsla,  // standard button / input fill
    pub control_stroke: Hsla,
    pub flat: Hsla,     // GNOME flat pill button
    // text
    pub text: Hsla,
    pub text2: Hsla,
    pub text3: Hsla,
    pub accent: Hsla,
    pub on_accent: Hsla,
    // status
    pub ok: Hsla,
    pub warn: Hsla,
    pub bad: Hsla,
    pub ok_bg: Hsla,
    pub warn_bg: Hsla,
    pub bad_bg: Hsla,
    pub info_bg: Hsla,
    // metrics
    pub radius: f32,      // controls
    pub card_radius: f32, // cards / lists
    pub row_h: f32,
    pub nav_w: f32,
}

impl Pal {
    pub fn new(platform: Platform, dark: bool) -> Pal {
        match (platform, dark) {
            (Platform::Windows, false) => Pal {
                platform, dark, font: "Segoe UI", mono: "Cascadia Mono", text_size: 14., small_size: 12.,
                bg: c(0xF3F3F3), layer: c(0xFBFBFB), side: c(0xF3F3F3), card: c(0xFFFFFF), stroke: c(0xE5E5E5), sep: c(0xE5E5E5),
                nav_sel: c(0xEAEAEA), hover: c(0xF5F5F5), control: c(0xFFFFFF), control_stroke: c(0xD1D1D1), flat: c(0xF0F0F0),
                text: c(0x1B1B1B), text2: c(0x616161), text3: c(0x8A8A8A), accent: c(0x005FB8), on_accent: c(0xFFFFFF),
                ok: c(0x0F7B0F), warn: c(0x9D5D00), bad: c(0xC42B1C), ok_bg: c(0xDFF6DD), warn_bg: c(0xFFF4CE), bad_bg: c(0xFDE7E9), info_bg: c(0xF6F6F6),
                radius: 4., card_radius: 4., row_h: 68., nav_w: 280.,
            },
            (Platform::Windows, true) => Pal {
                platform, dark, font: "Segoe UI", mono: "Cascadia Mono", text_size: 14., small_size: 12.,
                bg: c(0x202020), layer: c(0x272727), side: c(0x202020), card: c(0x2B2B2B), stroke: c(0x1D1D1D), sep: c(0x1D1D1D),
                nav_sel: c(0x2D2D2D), hover: c(0x323232), control: c(0x2D2D2D), control_stroke: c(0x3A3A3A), flat: c(0x3A3A3A),
                text: c(0xFFFFFF), text2: c(0xC5C5C5), text3: c(0x9A9A9A), accent: c(0x60CDFF), on_accent: c(0x000000),
                ok: c(0x6CCB5F), warn: c(0xFCE100), bad: c(0xFF99A4), ok_bg: c(0x393D1B), warn_bg: c(0x433519), bad_bg: c(0x442726), info_bg: c(0x2B2B2B),
                radius: 4., card_radius: 4., row_h: 68., nav_w: 280.,
            },
            (Platform::Mac, false) => Pal {
                platform, dark, font: ".SystemUIFont", mono: "Menlo", text_size: 13., small_size: 11.,
                bg: c(0xFFFFFF), layer: c(0xFFFFFF), side: c(0xF2F2F4), card: c(0xFFFFFF), stroke: c(0xE3E3E6), sep: c(0xE5E5EA),
                nav_sel: c(0xDCDCE0), hover: c(0xEDEDF0), control: c(0xFFFFFF), control_stroke: c(0xD1D1D6), flat: c(0xE8E8ED),
                text: c(0x1D1D1F), text2: c(0x6E6E73), text3: c(0x8E8E93), accent: c(0x0A84FF), on_accent: c(0xFFFFFF),
                ok: c(0x28A745), warn: c(0xC27C0E), bad: c(0xE5484D), ok_bg: c(0xE3F5E7), warn_bg: c(0xFFF3D6), bad_bg: c(0xFDE8E8), info_bg: c(0xF2F2F4),
                radius: 6., card_radius: 10., row_h: 52., nav_w: 220.,
            },
            (Platform::Mac, true) => Pal {
                platform, dark, font: ".SystemUIFont", mono: "Menlo", text_size: 13., small_size: 11.,
                bg: c(0x1E1E1E), layer: c(0x1E1E1E), side: c(0x2A2A2C), card: c(0x2C2C2E), stroke: c(0x3A3A3C), sep: c(0x3A3A3C),
                nav_sel: c(0x3A3A3C), hover: c(0x333335), control: c(0x3A3A3C), control_stroke: c(0x48484A), flat: c(0x3A3A3C),
                text: c(0xFFFFFF), text2: c(0x98989D), text3: c(0x8E8E93), accent: c(0x0A84FF), on_accent: c(0xFFFFFF),
                ok: c(0x32D74B), warn: c(0xFFD60A), bad: c(0xFF453A), ok_bg: c(0x1F3A26), warn_bg: c(0x3D3416), bad_bg: c(0x3D1F1E), info_bg: c(0x2C2C2E),
                radius: 6., card_radius: 10., row_h: 52., nav_w: 220.,
            },
            (Platform::Gnome, false) => Pal {
                platform, dark, font: "Cantarell", mono: "Source Code Pro", text_size: 15., small_size: 13.,
                bg: c(0xFAFAFA), layer: c(0xFAFAFA), side: c(0xEBEBED), card: c(0xFFFFFF), stroke: c(0xE0E0E0), sep: c(0xEBEBEB),
                nav_sel: c(0xDEDEE0), hover: c(0xEDEDED), control: c(0xFFFFFF), control_stroke: c(0xD0D0D0), flat: c(0xECECEC),
                text: c(0x1E1E1E), text2: c(0x6E6E6E), text3: c(0x8A8A8A), accent: c(0x3584E4), on_accent: c(0xFFFFFF),
                ok: c(0x26A269), warn: c(0xAE7B03), bad: c(0xC01C28), ok_bg: c(0xE1F4EA), warn_bg: c(0xFBF0D5), bad_bg: c(0xF8E1E3), info_bg: c(0xFFFFFF),
                radius: 8., card_radius: 12., row_h: 64., nav_w: 260.,
            },
            (Platform::Gnome, true) => Pal {
                platform, dark, font: "Cantarell", mono: "Source Code Pro", text_size: 15., small_size: 13.,
                bg: c(0x242424), layer: c(0x242424), side: c(0x303030), card: c(0x383838), stroke: c(0x2E2E2E), sep: c(0x2E2E2E),
                nav_sel: c(0x454545), hover: c(0x3A3A3A), control: c(0x383838), control_stroke: c(0x4A4A4A), flat: c(0x454545),
                text: c(0xFFFFFF), text2: c(0xA0A0A0), text3: c(0x8A8A8A), accent: c(0x3584E4), on_accent: c(0xFFFFFF),
                ok: c(0x8FF0A4), warn: c(0xF8E45C), bad: c(0xFF7B63), ok_bg: c(0x26382D), warn_bg: c(0x3D3A1F), bad_bg: c(0x4A2424), info_bg: c(0x383838),
                radius: 8., card_radius: 12., row_h: 64., nav_w: 260.,
            },
        }
    }

    /// Brand colour for a forge logo (GitHub's mark inverts in dark mode).
    pub fn logo_color(&self, forge: rebro_core::Forge) -> Hsla {
        match forge {
            rebro_core::Forge::GitHub => if self.dark { c(0xFFFFFF) } else { c(0x181717) },
            rebro_core::Forge::GitLab => c(0xFC6D26),
            rebro_core::Forge::Forgejo => c(0x2185D0),
            rebro_core::Forge::Gitea => c(0x609926),
        }
    }
}
