//! Toolbar / tool window icons drawn as vector paths (IntelliJ New UI style line icons on a
//! 16×16 grid, 1.5px strokes): crisp at any scale, colored by the theme, no image assets.
use gpui::{App, AnyView, Bounds, Hsla, IntoElement, PathBuilder, Pixels, Point, SharedString, Styled, Window, canvas, div, point, prelude::*, px};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Ico {
    Folder,
    Branch,
    Play,
    Bug,
    Stop,
    Terminal,
    Commit,
    Pull,
    Push,
    Search,
    Gear,
    Maven,
    Problems,
    ChevronDown,
    ChevronRight,
    ChevronLeft,
    Undo,
    ChevronUp,
    Plus,
    Close,
    Eye,
    Locate,
    Minimize,
    Run,
    Refresh,
    Minus,
    Check,
    Stash,
    List,
    Tree,
    History,
    Database,
}

enum Shape {
    /// open polyline
    Line(Vec<(f32, f32)>),
    /// closed outline
    Loop(Vec<(f32, f32)>),
    /// filled polygon
    Fill(Vec<(f32, f32)>),
    Circle(f32, f32, f32),
    Dot(f32, f32, f32),
}

fn ring(cx: f32, cy: f32, r: f32) -> Vec<(f32, f32)> {
    let n = 20;
    (0..n).map(|i| {
        let a = i as f32 / n as f32 * std::f32::consts::TAU;
        (cx + r * a.cos(), cy + r * a.sin())
    })
    .collect()
}

fn shapes(i: Ico) -> Vec<Shape> {
    use Shape::*;
    match i {
        Ico::Folder => vec![Loop(vec![(1.5, 3.5), (6., 3.5), (7.5, 5.), (14.5, 5.), (14.5, 13.), (1.5, 13.)])],
        Ico::Branch => vec![
            Circle(4.5, 3.5, 1.7),
            Circle(4.5, 12.5, 1.7),
            Circle(11.5, 4.5, 1.7),
            Line(vec![(4.5, 5.2), (4.5, 10.8)]),
            Line(vec![(11.5, 6.2), (11.5, 7.5), (10.5, 9.), (6., 10.), (4.5, 10.8)]),
        ],
        Ico::Play | Ico::Run => vec![Fill(vec![(4.5, 2.8), (13., 8.), (4.5, 13.2)])],
        Ico::Bug => vec![
            Loop(ring(8., 9.5, 3.6)),
            Circle(8., 4.6, 1.8),
            Line(vec![(8., 6.5), (8., 13.)]),
            Line(vec![(1.8, 7.), (4.5, 8.2)]),
            Line(vec![(1.8, 10.), (4.4, 10.)]),
            Line(vec![(2.2, 13.5), (4.8, 12.)]),
            Line(vec![(14.2, 7.), (11.5, 8.2)]),
            Line(vec![(14.2, 10.), (11.6, 10.)]),
            Line(vec![(13.8, 13.5), (11.2, 12.)]),
        ],
        Ico::Stop => vec![Fill(vec![(4., 4.), (12., 4.), (12., 12.), (4., 12.)])],
        Ico::Terminal => vec![Loop(vec![(1.5, 2.5), (14.5, 2.5), (14.5, 13.5), (1.5, 13.5)]), Line(vec![(4., 6.), (6.5, 8.), (4., 10.)]), Line(vec![(8., 10.5), (12., 10.5)])],
        Ico::Commit => vec![Circle(8., 8., 3.), Line(vec![(1., 8.), (5., 8.)]), Line(vec![(11., 8.), (15., 8.)])],
        Ico::Pull => vec![Line(vec![(8., 1.5), (8., 11.5)]), Line(vec![(4., 7.5), (8., 11.5), (12., 7.5)]), Line(vec![(3., 14.5), (13., 14.5)])],
        Ico::Push => vec![Line(vec![(8., 14.5), (8., 4.5)]), Line(vec![(4., 8.5), (8., 4.5), (12., 8.5)]), Line(vec![(3., 1.5), (13., 1.5)])],
        Ico::Search => vec![Circle(7., 7., 4.7), Line(vec![(10.4, 10.4), (14.2, 14.2)])],
        Ico::Gear => {
            let mut v = vec![Circle(8., 8., 2.2), Loop(ring(8., 8., 5.))];
            for k in 0..8 {
                let a = k as f32 / 8. * std::f32::consts::TAU;
                v.push(Line(vec![(8. + 5. * a.cos(), 8. + 5. * a.sin()), (8. + 6.8 * a.cos(), 8. + 6.8 * a.sin())]));
            }
            v
        }
        Ico::Maven => vec![
            Line(vec![(2.5, 13.), (2.5, 5.5)]),
            Line(vec![(2.5, 7.), (4., 5.3), (6., 5.3), (7.5, 7.), (7.5, 13.)]),
            Line(vec![(7.5, 7.), (9., 5.3), (11., 5.3), (12.8, 7.), (12.8, 13.)]),
        ],
        Ico::Problems => vec![Loop(vec![(8., 1.8), (14.6, 13.6), (1.4, 13.6)]), Line(vec![(8., 6.), (8., 9.6)]), Dot(8., 11.6, 0.9)],
        Ico::ChevronDown => vec![Line(vec![(4., 6.), (8., 10.), (12., 6.)])],
        Ico::ChevronUp => vec![Line(vec![(4., 10.), (8., 6.), (12., 10.)])],
        Ico::ChevronRight => vec![Line(vec![(6., 4.), (10., 8.), (6., 12.)])],
        Ico::Undo => vec![Line(vec![(5., 3.), (2., 6.5), (5., 10.)]), Line(vec![(2., 6.5), (9., 6.5), (13., 8.5), (13., 12.5)])],
        Ico::ChevronLeft => vec![Line(vec![(10., 4.), (6., 8.), (10., 12.)])],
        Ico::Plus => vec![Line(vec![(8., 3.), (8., 13.)]), Line(vec![(3., 8.), (13., 8.)])],
        Ico::Close => vec![Line(vec![(4., 4.), (12., 12.)]), Line(vec![(12., 4.), (4., 12.)])],
        Ico::Eye => vec![Loop(vec![(1.2, 8.), (3.5, 5.), (8., 3.5), (12.5, 5.), (14.8, 8.), (12.5, 11.), (8., 12.5), (3.5, 11.)]), Circle(8., 8., 2.2)],
        Ico::Locate => vec![
            Circle(8., 8., 4.6),
            Dot(8., 8., 1.4),
            Line(vec![(8., 0.8), (8., 3.)]),
            Line(vec![(8., 13.), (8., 15.2)]),
            Line(vec![(0.8, 8.), (3., 8.)]),
            Line(vec![(13., 8.), (15.2, 8.)]),
        ],
        Ico::Minimize => vec![Line(vec![(3.5, 8.), (12.5, 8.)])],
        Ico::Refresh => {
            // a 300° arc with an arrow head at its open end
            let arc: Vec<(f32, f32)> = (0..=16).map(|k| {
                let a = (-60. + 300. * k as f32 / 16.).to_radians();
                (8. + 5.2 * a.cos(), 8. + 5.2 * a.sin())
            }).collect();
            vec![Line(arc), Line(vec![(10.6, 1.6), (10.6, 3.6), (12.6, 3.6)])]
        }
        Ico::Minus => vec![Line(vec![(3., 8.), (13., 8.)])],
        Ico::Check => vec![Line(vec![(3., 8.5), (6.5, 12.), (13., 4.5)])],
        Ico::Stash => vec![
            Loop(vec![(1.5, 8.5), (5., 8.5), (6., 10.5), (10., 10.5), (11., 8.5), (14.5, 8.5), (14.5, 14.), (1.5, 14.)]),
            Line(vec![(1.5, 8.5), (3.5, 3.), (12.5, 3.), (14.5, 8.5)]),
        ],
        Ico::List => vec![Line(vec![(2., 4.), (14., 4.)]), Line(vec![(2., 8.), (14., 8.)]), Line(vec![(2., 12.), (14., 12.)])],
        Ico::Tree => vec![
            Line(vec![(2., 3.), (8., 3.)]),
            Line(vec![(4., 3.), (4., 12.), (8., 12.)]),
            Line(vec![(4., 7.5), (8., 7.5)]),
            Line(vec![(10., 7.5), (14., 7.5)]),
            Line(vec![(10., 12.), (14., 12.)]),
        ],
        Ico::Database => vec![
            Loop(vec![(2.5, 3.8), (4.5, 2.4), (8., 1.9), (11.5, 2.4), (13.5, 3.8), (11.5, 5.2), (8., 5.7), (4.5, 5.2)]),
            Line(vec![(2.5, 3.8), (2.5, 12.2), (4.5, 13.6), (8., 14.1), (11.5, 13.6), (13.5, 12.2), (13.5, 3.8)]),
            Line(vec![(2.5, 8.), (4.5, 9.4), (8., 9.9), (11.5, 9.4), (13.5, 8.)]),
        ],
        Ico::History => vec![Loop(ring(8., 8., 6.)), Line(vec![(8., 4.5), (8., 8.), (10.8, 9.8)])],
    }
}

/// A `size`-pixel icon in `color`.
pub fn icon(i: Ico, size: f32, color: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |b: Bounds<Pixels>, _, window, _| {
            let s = f32::from(b.size.width) / 16.;
            let pt = |(x, y): (f32, f32)| -> Point<Pixels> { point(b.origin.x + px(x * s), b.origin.y + px(y * s)) };
            let stroke = px((1.5 * s).max(1.1));
            let draw = |pts: &[(f32, f32)], closed: bool, fill: bool, window: &mut Window| {
                let mut pb = if fill { PathBuilder::fill() } else { PathBuilder::stroke(stroke) };
                pb.move_to(pt(pts[0]));
                for p in &pts[1..] {
                    pb.line_to(pt(*p));
                }
                if closed || fill {
                    pb.close();
                }
                if let Ok(path) = pb.build() {
                    window.paint_path(path, color);
                }
            };
            for sh in shapes(i) {
                match sh {
                    Shape::Line(v) => draw(&v, false, false, window),
                    Shape::Loop(v) => draw(&v, true, false, window),
                    Shape::Fill(v) => draw(&v, true, true, window),
                    Shape::Circle(x, y, r) => draw(&ring(x, y, r), true, false, window),
                    Shape::Dot(x, y, r) => draw(&ring(x, y, r), true, true, window),
                }
            }
        },
    )
    .size(px(size))
    .flex_none()
}

/// A plain text tooltip (hover a toolbar / stripe button).
pub struct Tip(SharedString);

impl Render for Tip {
    fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        use crate::theme::{self, c};
        div()
            .px_2()
            .py_1()
            .bg(c(theme::POPUP_BG))
            .border_1()
            .border_color(c(theme::BORDER))
            .rounded_md()
            .shadow_md()
            .text_size(px(12.))
            .text_color(c(theme::FG))
            .child(self.0.clone())
    }
}

/// `.tooltip(tip("…"))`
pub fn tip(text: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let t: SharedString = text.into();
    move |_, cx| cx.new(|_| Tip(t.clone())).into()
}
