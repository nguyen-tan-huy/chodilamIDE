//! Colors. The constants are keys (their values are the old Darcula palette); each
//! theme is a remap table applied inside `c()`, so a theme switch is a global swap +
//! repaint and no call site has to know which theme is active.
//!
//! Just two looks: dark with light text and light with black text (Apple system
//! grays + #0a84ff accent), optionally transparent over the desktop wallpaper.
use gpui::{Hsla, rgb};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use syntax::Style;

pub const THEMES: &[&str] = &["Dark", "Light"];

/// Editor color scheme choices: "Theo theme app" follows the app theme.
pub const EDITOR_FOLLOW: &str = "Theo theme app";

static ACTIVE: AtomicU8 = AtomicU8::new(0);
/// editor scheme index + 1 (0 = follow the app theme)
static EDITOR: AtomicU8 = AtomicU8::new(0);

fn index_of(name: &str) -> Option<usize> {
    THEMES.iter().position(|t| t.eq_ignore_ascii_case(name))
}

/// Index into THEMES for a saved name; themes of earlier versions ("Dracula",
/// "Solarized Light", ...) get the look with the same brightness.
pub fn theme_index(name: &str) -> usize {
    index_of(name).unwrap_or_else(|| usize::from(name.to_ascii_lowercase().contains("light")))
}

/// Selects a theme by name (see `theme_index`).
pub fn set_theme(name: &str) {
    ACTIVE.store(theme_index(name) as u8, Ordering::Relaxed);
}

/// Selects the editor color scheme ("Theo theme app" = follow the app theme).
pub fn set_editor_theme(name: &str) {
    let i = if name == EDITOR_FOLLOW { 0 } else { theme_index(name) as u8 + 1 };
    EDITOR.store(i, Ordering::Relaxed);
}

static GLASS: AtomicBool = AtomicBool::new(true);

/// Transparent mode: no background behind panels, tabs, bars or buttons, so they
/// float on the desktop wallpaper. Popups and dialogs keep their solid background.
pub fn set_glass(on: bool) {
    GLASS.store(on, Ordering::Relaxed);
}

pub fn glass_on() -> bool {
    GLASS.load(Ordering::Relaxed)
}

/// App (chrome) color.
pub fn c(hex: u32) -> Hsla {
    let i = ACTIVE.load(Ordering::Relaxed) as usize;
    glass(hex, i).unwrap_or_else(|| rgb(remap(hex, i)).into())
}

/// The transparent-mode version of a surface / highlight color, None for solid
/// colors (text, icons, syntax, popups). Highlights become tints of the text or
/// accent color so they read the same over any wallpaper.
fn glass(hex: u32, theme: usize) -> Option<Hsla> {
    if !GLASS.load(Ordering::Relaxed) {
        return None;
    }
    let dark = theme == 0;
    let with = |key: u32, a: f32| {
        let mut h: Hsla = rgb(remap(key, theme)).into();
        h.a = a;
        Some(h)
    };
    match hex {
        // no background: the frame, editor, tree and tool panels show the wallpaper
        FRAME_BG | BG => with(BG, 0.),
        // a faint line keeps the blocks apart without filling them
        BORDER => with(FG, if dark { 0.22 } else { 0.18 }),
        INDENT_GUIDE => with(FG, 0.08),
        CURRENT_LINE => with(FG, 0.06),
        ROW_HOVER => with(FG, 0.10),
        ROW_ACTIVE => with(ACCENT, if dark { 0.55 } else { 0.30 }),
        SELECTION => with(ACCENT, if dark { 0.40 } else { 0.28 }),
        FIND_MATCH => with(WARNING, 0.30),
        EXEC_LINE => with(RUN_GREEN, 0.22),
        // grey text washes out over the wallpaper: secondary text stays close to the text color
        FG_DIM | INLINE_VALUE | GUTTER_FG => with(FG, 0.72),
        GUTTER_FG_ACTIVE => with(FG, 0.9),
        _ => None,
    }
}

/// Editor color: the editor scheme when one is chosen, else the app theme.
pub fn ed(hex: u32) -> Hsla {
    let i = match EDITOR.load(Ordering::Relaxed) {
        0 => ACTIVE.load(Ordering::Relaxed) as usize,
        n => n as usize - 1,
    };
    glass(hex, i).unwrap_or_else(|| rgb(remap(hex, i)).into())
}

fn remap(hex: u32, theme: usize) -> u32 {
    let Some(table) = tables().get(theme) else { return hex };
    table.iter().find(|(from, _)| *from == hex).map(|(_, to)| *to).unwrap_or(hex)
}

pub const BG: u32 = 0x1e1f22;
pub const PANEL_BG: u32 = 0x2b2d30;
pub const BORDER: u32 = 0x393b40;
pub const FG: u32 = 0xbcbec4;
pub const FG_DIM: u32 = 0x6f737a;
pub const GUTTER_FG: u32 = 0x4b5059;
pub const GUTTER_FG_ACTIVE: u32 = 0xa1a3ab;
pub const CURRENT_LINE: u32 = 0x26282e;
pub const SELECTION: u32 = 0x214283;
pub const CURSOR: u32 = 0xced0d6;
pub const ROW_HOVER: u32 = 0x35373b;
pub const ROW_ACTIVE: u32 = 0x2e436e;
pub const ACCENT: u32 = 0x3574f0;
pub const MODIFIED: u32 = 0x6aab73;
pub const FIND_MATCH: u32 = 0x3b514d;
pub const ERROR: u32 = 0xf75464;
pub const WARNING: u32 = 0xe0b050;
pub const POPUP_BG: u32 = 0x2b2d30;
pub const BREAKPOINT: u32 = 0xdb5c5c;
pub const BREAKPOINT_COND: u32 = 0xe08a3c;
pub const EXEC_LINE: u32 = 0x2d4a36;
pub const RUN_GREEN: u32 = 0x5fb865;
pub const INDENT_GUIDE: u32 = 0x313438;
pub const INLINE_VALUE: u32 = 0x8a8f99;
// git file status (IntelliJ Darcula)
/// IntelliJ "Islands": the frame behind the rounded editor / tool window islands
pub const FRAME_BG: u32 = 0x161719;
/// corner radius of an island
pub const ISLAND_RADIUS: f32 = 18.;
/// gap between islands (also the resize handles)
pub const ISLAND_GAP: f32 = 12.;
pub const GIT_MODIFIED: u32 = 0x6897bb;
pub const GIT_ADDED: u32 = 0x629755;
pub const GIT_UNTRACKED: u32 = 0xd1675a;

// syntax (Darcula)
const KW: u32 = 0xcf8e6d;
const FUNC: u32 = 0x56a8f5;
const NUM: u32 = 0x2aacb8;
const COMMENT: u32 = 0x7a7e85;
const CONST: u32 = 0xc77dbb;
const ATTR: u32 = 0xb3ae60;
const TAG: u32 = 0xd5b778;

/// One theme's colors, in the order of `palette()`.
struct P {
    bg: u32, panel: u32, border: u32, fg: u32, dim: u32, gutter: u32, gutter_active: u32, cur: u32, sel: u32,
    cursor: u32, hover: u32, active: u32, accent: u32, find: u32, err: u32, warn: u32, exec: u32, green: u32,
    guide: u32, kw: u32, func: u32, str: u32, num: u32, comment: u32, cons: u32, attr: u32, tag: u32, punct: u32,
}

/// `hex` moved toward black by `f` (0..1).
fn darken(hex: u32, f: f32) -> u32 {
    let ch = |shift: u32| ((((hex >> shift) & 0xff) as f32) * (1. - f)).round() as u32;
    (ch(16) << 16) | (ch(8) << 8) | ch(0)
}

fn luma(hex: u32) -> f32 {
    (0.299 * ((hex >> 16) & 0xff) as f32 + 0.587 * ((hex >> 8) & 0xff) as f32 + 0.114 * (hex & 0xff) as f32) / 255.
}

/// Darcula constant → this theme's color.
fn palette(p: P) -> Vec<(u32, u32)> {
    // the island frame: a bit darker than the darker of editor / panel (light themes: a light grey)
    let base = if luma(p.bg) < luma(p.panel) { p.bg } else { p.panel };
    let frame = if luma(base) > 0.5 { darken(base, 0.07) } else { darken(base, 0.28) };
    vec![
        (FRAME_BG, frame),
        (BG, p.bg), (PANEL_BG, p.panel), (BORDER, p.border), (FG, p.fg), (FG_DIM, p.dim), (GUTTER_FG, p.gutter),
        (GUTTER_FG_ACTIVE, p.gutter_active), (CURRENT_LINE, p.cur), (SELECTION, p.sel), (CURSOR, p.cursor),
        (ROW_HOVER, p.hover), (ROW_ACTIVE, p.active), (ACCENT, p.accent), (FIND_MATCH, p.find), (ERROR, p.err),
        (WARNING, p.warn), (EXEC_LINE, p.exec), (RUN_GREEN, p.green), (MODIFIED, p.green), (INDENT_GUIDE, p.guide),
        (INLINE_VALUE, p.dim), (KW, p.kw), (FUNC, p.func), (NUM, p.num), (COMMENT, p.comment), (CONST, p.cons),
        (ATTR, p.attr), (TAG, p.tag), (0x8c8f96, p.dim),
        // STR == MODIFIED and PUNCT == GUTTER_FG_ACTIVE in Darcula: first match wins, so the
        // syntax roles get their own keys through syntax() below
        (STR_KEY, p.str), (PUNCT_KEY, p.punct),
    ]
}

// syntax roles whose Darcula color collides with a UI constant get private keys
const STR_KEY: u32 = 0x01_6aab73;
const PUNCT_KEY: u32 = 0x01_a1a3ab;

fn tables() -> &'static Vec<Vec<(u32, u32)>> {
    static T: std::sync::OnceLock<Vec<Vec<(u32, u32)>>> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        vec![
            // Dark glass: #1c1c1e surface, #2c2c2e raised, #3a3a3c separator; Xcode Default (Dark) syntax
            palette(P { bg: 0x1c1c1e, panel: 0x2c2c2e, border: 0x3a3a3c, fg: 0xf5f5f7, dim: 0x98989d, gutter: 0x636366, gutter_active: 0xaeaeb2,
                cur: 0x242426, sel: 0x1f3a5f, cursor: 0xf5f5f7, hover: 0x3a3a3c, active: 0x0a3d73, accent: 0x0a84ff, find: 0x4a4120,
                err: 0xff453a, warn: 0xffd60a, exec: 0x1d3a26, green: 0x30d158, guide: 0x2c2c2e, kw: 0xfc5fa3, func: 0x67b7a4,
                str: 0xfc6a5d, num: 0xd0bf69, comment: 0x6c7986, cons: 0xa167e6, attr: 0xfd8f3f, tag: 0x5dd8ff, punct: 0xd1d1d6 }),
            // Light glass: #ffffff editor on #f5f5f7, #d2d2d7 separator; Xcode Default (Light) syntax
            palette(P { bg: 0xffffff, panel: 0xf5f5f7, border: 0xd2d2d7, fg: 0x1d1d1f, dim: 0x6e6e73, gutter: 0xa1a1a6, gutter_active: 0x48484a,
                cur: 0xf5f5f7, sel: 0xb3d7ff, cursor: 0x1d1d1f, hover: 0xe8e8ed, active: 0xd6e8ff, accent: 0x0a84ff, find: 0xfff2a8,
                err: 0xd70015, warn: 0xb25000, exec: 0xdcf5e2, green: 0x248a3d, guide: 0xebebf0, kw: 0x9b2393, func: 0x326d74,
                str: 0xc41a16, num: 0x1c00cf, comment: 0x5d6c79, cons: 0x6c36a9, attr: 0x643820, tag: 0x0b4f79, punct: 0x1d1d1f }),
        ]
    })
}

/// Syntax color for a role whose Darcula key collides with a UI constant.
fn syn(key: u32) -> Hsla {
    let i = match EDITOR.load(Ordering::Relaxed) {
        0 => ACTIVE.load(Ordering::Relaxed) as usize,
        n => n as usize - 1,
    };
    rgb(remap(key, i)).into()
}

pub fn syntax(style: Style) -> Hsla {
    match style {
        Style::String => syn(STR_KEY),
        Style::Punctuation => syn(PUNCT_KEY),
        s => ed(match s {
            Style::Keyword => KW,
            Style::Function => FUNC,
            Style::Number => NUM,
            Style::Comment => COMMENT,
            Style::Constant | Style::Property => CONST,
            Style::Attribute => ATTR,
            Style::Tag => TAG,
            _ => FG,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remap_by_theme() {
        assert_eq!(remap(BG, 0), 0x1c1c1e);
        assert_eq!(remap(BG, 1), 0xffffff);
        assert_eq!(remap(0x123456, 1), 0x123456); // unknown colors pass through
        assert_eq!(tables().len(), THEMES.len());
        // themes of earlier versions land on the matching brightness
        assert_eq!(theme_index("Dracula"), 0);
        assert_eq!(theme_index("Solarized Light"), 1);
        // editor scheme separate from the app theme
        set_glass(false);
        set_theme("Light");
        set_editor_theme("Dark");
        assert_eq!(ed(BG), rgb(0x1c1c1e).into());
        assert_eq!(c(BG), rgb(0xffffff).into());
        assert_eq!(syntax(Style::String), rgb(0xfc6a5d).into());
        set_editor_theme(EDITOR_FOLLOW);
        assert_eq!(ed(BG), rgb(0xffffff).into());
        set_glass(true);
        assert!(c(BG).a < 1.0 && c(FG).a == 1.0);
        set_theme("Dark");
    }
}
