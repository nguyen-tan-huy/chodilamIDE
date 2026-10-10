//! Drag-to-resize for the project tree, the bottom tool window and the Maven/dependency
//! split. Sizes are saved per project (.nvim/java-debug-model/chodilamide-layout.json).
use crate::theme::{self, c};
use crate::workspace::Workspace;
use gpui::{Context, CursorStyle, Div, MouseButton, MouseMoveEvent, Pixels, Stateful, div, prelude::*, px};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum DragKind {
    Tree,
    Panel,
    /// run panel: stack frames | variables | test tree widths
    Frames,
    /// the tool panel's width when it sits left/right of the editor
    PanelSide,
    Vars,
    TestTree,
}

#[derive(Clone, Copy, Debug)]
pub struct Drag {
    kind: DragKind,
    start: Pixels,
    start_size: f32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Sizes {
    pub tree: f32,
    pub panel: f32,
    pub maven_split: f32,
    #[serde(default = "d_panel_w")]
    pub panel_w: f32,
    #[serde(default = "d_frames")]
    pub frames: f32,
    #[serde(default = "d_vars")]
    pub vars: f32,
    #[serde(default = "d_test_tree")]
    pub test_tree: f32,
}

fn d_panel_w() -> f32 {
    560.
}
fn d_frames() -> f32 {
    300.
}
fn d_vars() -> f32 {
    420.
}
fn d_test_tree() -> f32 {
    460.
}

fn file(ws: &Workspace) -> std::path::PathBuf {
    project_model::cache::state_dir(&ws.root_str()).join("chodilamide-layout.json")
}

impl Sizes {
    pub fn clamp(self) -> Self {
        Self {
            tree: self.tree.clamp(140., 900.),
            panel: self.panel.clamp(90., 1400.),
            maven_split: self.maven_split.clamp(200., 1600.),
            panel_w: self.panel_w.clamp(200., 2000.),
            frames: self.frames.clamp(120., 1400.),
            vars: self.vars.clamp(120., 1400.),
            test_tree: self.test_tree.clamp(120., 1400.),
        }
    }
}

impl Workspace {
    pub(crate) fn load_sizes(&mut self) {
        let defaults = Sizes { tree: self.settings.tree_width, panel: self.settings.panel_height, maven_split: 520., panel_w: d_panel_w(), frames: d_frames(), vars: d_vars(), test_tree: d_test_tree() };
        self.sizes = std::fs::read_to_string(file(self)).ok().and_then(|s| serde_json::from_str::<Sizes>(&s).ok()).unwrap_or(defaults).clamp();
    }

    fn save_sizes(&self) {
        if let Ok(bytes) = serde_json::to_vec(&self.sizes) {
            let _ = project_model::cache::write_atomic(&file(self), &bytes);
        }
    }

    /// A thin handle; dragging it resizes `kind`.
    pub(crate) fn resize_handle(&self, kind: DragKind, cx: &mut Context<Self>) -> Stateful<Div> {
        let vertical = kind != DragKind::Panel;
        let active = self.drag.is_some_and(|d| d.kind == kind);
        div()
            .id(match kind {
                DragKind::Tree => "resize-tree",
                DragKind::Panel => "resize-panel",
                DragKind::Frames => "resize-frames",
                DragKind::PanelSide => "resize-panel-side",
                DragKind::Vars => "resize-vars",
                DragKind::TestTree => "resize-test-tree",
            })
            .flex_none()
            .when(vertical, |d| d.w(px(5.)).h_full().cursor(CursorStyle::ResizeLeftRight))
            .when(!vertical, |d| d.h(px(5.)).w_full().cursor(CursorStyle::ResizeUpDown))
            .when(active, |d| d.bg(c(theme::ACCENT)))
            .hover(|s| s.bg(c(theme::ACCENT)))
            .on_mouse_down(MouseButton::Left, cx.listener(move |ws, ev: &gpui::MouseDownEvent, _, cx| {
                cx.stop_propagation();
                let (start, start_size) = match kind {
                    DragKind::Tree => (ev.position.x, ws.sizes.tree),
                    DragKind::Panel => (ev.position.y, ws.sizes.panel),
                    DragKind::Frames => (ev.position.x, ws.sizes.frames),
                    DragKind::PanelSide => (ev.position.x, ws.sizes.panel_w),
                    DragKind::Vars => (ev.position.x, ws.sizes.vars),
                    DragKind::TestTree => (ev.position.x, ws.sizes.test_tree),
                };
                ws.drag = Some(Drag { kind, start, start_size });
                cx.notify()
            }))
    }

    /// Root mouse-move while dragging.
    pub(crate) fn on_drag_move(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(d) = self.drag else { return };
        if ev.pressed_button != Some(MouseButton::Left) {
            self.end_drag(cx);
            return;
        }
        let delta = |now: Pixels| f32::from(now - d.start);
        match d.kind {
            // a right-hand tree/panel grows when dragged to the left
            DragKind::Tree if self.settings.tree_side == "right" => self.sizes.tree = d.start_size - delta(ev.position.x),
            DragKind::Tree => self.sizes.tree = d.start_size + delta(ev.position.x),
            DragKind::PanelSide if self.settings.panel_side == "left" => self.sizes.panel_w = d.start_size + delta(ev.position.x),
            DragKind::PanelSide => self.sizes.panel_w = d.start_size - delta(ev.position.x),
            // the panel grows upward
            DragKind::Panel => self.sizes.panel = d.start_size - delta(ev.position.y),
            DragKind::Frames => self.sizes.frames = d.start_size + delta(ev.position.x),
            DragKind::Vars => self.sizes.vars = d.start_size + delta(ev.position.x),
            DragKind::TestTree => self.sizes.test_tree = d.start_size + delta(ev.position.x),
        }
        self.sizes = self.sizes.clamp();
        cx.notify();
    }

    pub(crate) fn end_drag(&mut self, cx: &mut Context<Self>) {
        if self.drag.take().is_some() {
            self.save_sizes();
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Sizes;

    #[test]
    fn sizes_are_clamped() {
        let s = Sizes { tree: 20., panel: 5000., maven_split: 300., panel_w: 560., frames: 1., vars: 420., test_tree: 460. }.clamp();
        assert_eq!(s.frames, 120.);
        // an old layout file without the run-panel widths still loads
        let old: Sizes = serde_json::from_str(r#"{"tree":300,"panel":200,"maven_split":520}"#).unwrap();
        assert_eq!((old.frames, old.vars, old.test_tree), (300., 420., 460.));
        assert_eq!((s.tree, s.panel, s.maven_split), (140., 1400., 300.));
    }
}
