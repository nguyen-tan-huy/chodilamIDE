//! Find Action (Ctrl+Shift+A) catalog and user key overrides.
//! Defaults are the `bind_keys` of each module; `~/.config/chodilamide/keymap.toml` replaces
//! the shortcuts of single actions (`"workspace::RunActive" = ["shift-f10"]`, `[]` = none).
use gpui::{App, DummyKeyboardMapper, Global, KeyBinding};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// An action shown in Find Action: GPUI action name, Vietnamese title, group.
pub struct Entry {
    pub name: &'static str,
    pub title: &'static str,
    pub group: &'static str,
}

const fn e(name: &'static str, title: &'static str, group: &'static str) -> Entry {
    Entry { name, title, group }
}

pub const CATALOG: &[Entry] = &[
    e("workspace::FindAction", "Tìm action (Find Action)", "Chung"),
    e("shell::OpenSettings", "Cài đặt", "Chung"),
    e("shell::NewProjectTab", "Mở project (tab mới)", "Project"),
    e("shell::CloseProjectTab", "Đóng project", "Project"),
    e("shell::NextProject", "Project kế tiếp", "Project"),
    e("shell::PrevProject", "Project trước", "Project"),
    e("shell::QuitApp", "Thoát ChoDiLamIDE", "Chung"),
    e("workspace::ReloadModel", "Reload Maven project", "Maven"),
    e("workspace::GoToFile", "Mở file theo tên (Go to File)", "Điều hướng"),
    e("workspace::GotoSymbol", "Tìm class / method (Go to Symbol)", "Điều hướng"),
    e("workspace::FindInFiles", "Tìm chữ trong project (Find in Files)", "Tìm kiếm"),
    e("workspace::NavBack", "Quay lại vị trí trước", "Điều hướng"),
    e("workspace::NavForward", "Tới vị trí sau", "Điều hướng"),
    e("workspace::Problems", "Danh sách lỗi (Problems)", "Code"),
    e("workspace::FocusTree", "Cây project", "Cửa sổ"),
    e("workspace::SelectOpenedFile", "Hiện file đang mở trong cây", "Cửa sổ"),
    e("workspace::SpringEndpoints", "Spring: Endpoints (mọi @GetMapping/@PostMapping…)", "Spring"),
    e("workspace::SpringBeans", "Spring: danh sách Beans", "Spring"),
    e("workspace::GotoBean", "Spring: tới bean được inject ở đây", "Spring"),
    e("workspace::JsfPageMap", "JSF: Page Map (template/include/component, trang thừa)", "JSF"),
    e("shell::ToggleProjectTree", "Ẩn/hiện cây project", "Bố cục"),
    e("shell::MoveProjectTree", "Chuyển cây project sang bên kia (trái ⇄ phải)", "Bố cục"),
    e("shell::PanelBottom", "Panel Run/Debug/Maven: đặt ở dưới", "Bố cục"),
    e("shell::PanelRight", "Panel Run/Debug/Maven: đặt bên phải", "Bố cục"),
    e("shell::PanelLeft", "Panel Run/Debug/Maven: đặt bên trái", "Bố cục"),
    e("workspace::TogglePanel", "Ẩn/hiện panel dưới", "Cửa sổ"),
    e("workspace::ToggleTerminal", "Terminal", "Cửa sổ"),
    e("workspace::CloseTab", "Đóng tab", "Cửa sổ"),
    e("workspace::NextTab", "Tab kế tiếp", "Cửa sổ"),
    e("workspace::PrevTab", "Tab trước", "Cửa sổ"),
    e("workspace::RunActive", "Run cấu hình đang chọn", "Run / Debug"),
    e("workspace::DebugActive", "Debug cấu hình đang chọn", "Run / Debug"),
    e("workspace::StopSession", "Dừng phiên chạy", "Run / Debug"),
    e("workspace::EditConfigurations", "Sửa cấu hình chạy (Edit Configurations)", "Run / Debug"),
    e("workspace::Resume", "Debug: tiếp tục (Resume)", "Run / Debug"),
    e("workspace::StepOver", "Debug: Step Over", "Run / Debug"),
    e("workspace::StepInto", "Debug: Step Into", "Run / Debug"),
    e("workspace::StepOut", "Debug: Step Out", "Run / Debug"),
    e("workspace::HotSwap", "Debug: nạp lại class (Hot Swap)", "Run / Debug"),
    e("workspace::EvaluateExpression", "Debug: tính biểu thức (Evaluate)", "Run / Debug"),
    e("lsp::ToggleBreakpoint", "Bật/tắt breakpoint", "Run / Debug"),
    e("lsp::EditBreakpoint", "Sửa điều kiện breakpoint", "Run / Debug"),
    e("lsp::GotoDefinition", "Tới định nghĩa / chỗ dùng (Ctrl+B)", "Code"),
    e("lsp::FindUsages", "Tìm chỗ dùng (Find Usages)", "Code"),
    e("lsp::Hover", "Xem tài liệu (Quick Documentation)", "Code"),
    e("lsp::ShowCompletion", "Gợi ý code", "Code"),
    e("lsp::ParameterInfo", "Danh sách tham số (Parameter Info)", "Code"),
    e("lsp::ShowCodeActions", "Sửa nhanh / Code actions (Alt+Enter)", "Code"),
    e("lsp::Rename", "Đổi tên (Rename)", "Refactor"),
    e("lsp::ExtractVariable", "Tách ra biến (Introduce Variable)", "Refactor"),
    e("lsp::ExtractConstant", "Tách ra hằng số (Introduce Constant)", "Refactor"),
    e("lsp::ExtractField", "Tách ra field (Introduce Field)", "Refactor"),
    e("lsp::ExtractMethod", "Tách ra method (Extract Method)", "Refactor"),
    e("lsp::IntroduceParameter", "Tách ra tham số (Introduce Parameter)", "Refactor"),
    e("lsp::FormatCode", "Format code", "Code"),
    e("lsp::OrganizeImports", "Sắp xếp import", "Code"),
    e("lsp::FileStructure", "Cấu trúc file (File Structure)", "Điều hướng"),
    e("lsp::NextError", "Lỗi kế tiếp", "Code"),
    e("lsp::PrevError", "Lỗi trước", "Code"),
    e("editor::Save", "Lưu file", "Sửa"),
    e("editor::Undo", "Hoàn tác (Undo)", "Sửa"),
    e("editor::Redo", "Làm lại (Redo)", "Sửa"),
    e("editor::DuplicateLine", "Nhân đôi dòng", "Sửa"),
    e("editor::DeleteLine", "Xóa dòng", "Sửa"),
    e("editor::SelectAll", "Chọn tất cả", "Sửa"),
    e("editor::Copy", "Copy", "Sửa"),
    e("editor::Cut", "Cut", "Sửa"),
    e("editor::Paste", "Paste", "Sửa"),
    e("editor::Find", "Tìm trong file", "Tìm kiếm"),
    e("editor::FindNext", "Tìm tiếp", "Tìm kiếm"),
    e("lsp::Generate", "Generate (getter/setter, constructor, toString…)", "Code"),
    e("lsp::GotoImplementation", "Tới implementation", "Điều hướng"),
    e("lsp::GotoSuper", "Tới class/method cha (Super)", "Điều hướng"),
    e("lsp::TypeHierarchy", "Type Hierarchy", "Điều hướng"),
    e("lsp::CallHierarchy", "Call Hierarchy (ai gọi method này)", "Điều hướng"),
    e("workspace::SearchEverywhere", "Tìm mọi nơi (Shift Shift)", "Điều hướng"),
    e("workspace::RunHttpRequest", "HTTP Client: chạy request ở con trỏ (.http)", "Run/Debug"),
    e("workspace::RunWithCoverage", "Run with Coverage (JaCoCo) - test đang mở / cấu hình đang chọn", "Run/Debug"),
    e("workspace::HideCoverage", "Ẩn coverage", "Run/Debug"),
    e("workspace::AttachJvm", "Debug: Attach vào JVM (Remote JVM Debug, cổng 5005)", "Run/Debug"),
    e("workspace::PauseSession", "Debug: Tạm dừng (Pause)", "Run/Debug"),
    e("workspace::ToggleBookmark", "Đặt / bỏ bookmark", "Điều hướng"),
    e("workspace::ShowBookmarks", "Danh sách bookmark", "Điều hướng"),
    e("workspace::TodoList", "TODO / FIXME trong project", "Code"),
    e("workspace::LocalHistory", "Local History của file", "Code"),
    e("workspace::GitCommit", "Git: Commit", "Git"),
    e("workspace::GitPush", "Git: Push", "Git"),
    e("workspace::GitPull", "Git: Pull", "Git"),
    e("workspace::GitLog", "Git: Log", "Git"),
    e("workspace::GitPanelToggle", "Git: cửa sổ Git (thay đổi, stash, xung đột)", "Git"),
    e("workspace::GitAnnotate", "Git: Annotate (blame)", "Git"),
    e("workspace::GitRollbackHunk", "Git: Rollback khối thay đổi ở con trỏ", "Git"),
    e("workspace::GitFileHistory", "Git: History file đang mở", "Git"),
    e("workspace::GitDiffFile", "Git: So sánh file với HEAD", "Git"),
    e("workspace::VcsPopup", "Git: menu (nhánh, commit, push…)", "Git"),
    e("db_view::DbToggle", "Database: MongoDB (kết nối, collection, document)", "Công cụ"),
    e("workspace::RecentFiles", "File mở gần đây (Recent Files)", "Điều hướng"),
    e("workspace::OpenReplace", "Thay thế trong file (Replace)", "Tìm kiếm"),
    e("workspace::ReplaceInFiles", "Thay thế trong project (Replace in Files)", "Tìm kiếm"),
    e("editor::CommentLine", "Comment/bỏ comment dòng", "Sửa"),
    e("editor::CommentBlock", "Comment khối /* */", "Sửa"),
    e("editor::MoveLineUp", "Chuyển dòng lên", "Sửa"),
    e("editor::MoveLineDown", "Chuyển dòng xuống", "Sửa"),
    e("editor::Unindent", "Bỏ thụt lề", "Sửa"),
    e("editor::JoinLines", "Nối dòng", "Sửa"),
    e("editor::ToggleCase", "Đổi HOA/thường", "Sửa"),
    e("editor::ExtendSelection", "Mở rộng vùng chọn (Extend Selection)", "Sửa"),
    e("editor::ShrinkSelection", "Thu hẹp vùng chọn", "Sửa"),
    e("editor::AddNextOccurrence", "Thêm con trỏ ở chỗ giống kế tiếp (Alt+J)", "Sửa"),
    e("editor::RemoveLastOccurrence", "Bỏ con trỏ vừa thêm", "Sửa"),
    e("editor::SelectAllOccurrences", "Con trỏ ở mọi chỗ giống", "Sửa"),
    e("editor::FoldCollapse", "Gập khối code (Collapse)", "Sửa"),
    e("editor::FoldExpand", "Mở khối code (Expand)", "Sửa"),
    e("editor::FoldAll", "Gập tất cả", "Sửa"),
    e("editor::UnfoldAll", "Mở tất cả", "Sửa"),
];

pub fn path() -> PathBuf {
    crate::settings::Settings::path().with_file_name("keymap.toml")
}

/// action name → shortcuts (each "ctrl-shift-a" or a chord "ctrl-k ctrl-c").
pub fn load_overrides() -> BTreeMap<String, Vec<String>> {
    std::fs::read_to_string(path()).ok().and_then(|s| toml::from_str(&s).ok()).unwrap_or_default()
}

pub fn save_override(name: &str, keys: Vec<String>) -> anyhow::Result<()> {
    let mut all = load_overrides();
    all.insert(name.to_string(), keys);
    let text = format!(
        "# ChoDiLamIDE - phím tắt riêng (Ctrl+Shift+A → chọn action → Ctrl+Enter để đổi)\n# \"tên action\" = [\"phím\", …]; [] = bỏ phím tắt\n{}",
        toml::to_string_pretty(&all)?
    );
    project_model::cache::write_atomic(&path(), text.as_bytes())
}

/// The bindings every module registered at startup, before user overrides.
struct Defaults(Vec<KeyBinding>);
impl Global for Defaults {}

/// Replaces the shortcuts of overridden actions (call after every `bind_keys`, and again after
/// an override changes). Errors = shortcuts that could not be parsed.
pub fn apply(cx: &mut App) -> Vec<String> {
    if !cx.has_global::<Defaults>() {
        let all: Vec<KeyBinding> = cx.key_bindings().borrow().bindings().cloned().collect();
        cx.set_global(Defaults(all));
    }
    let defaults = cx.global::<Defaults>().0.clone();
    let overrides = load_overrides();
    let mut errors = Vec::new();
    let mut out: Vec<KeyBinding> = defaults.iter().filter(|b| !overrides.contains_key(b.action().name())).cloned().collect();
    for (name, keys) in &overrides {
        let Ok(action) = cx.build_action(name, None) else {
            errors.push(format!("không có action '{name}'"));
            continue;
        };
        // keep the context of the default binding (e.g. "Editor" for editor commands)
        let ctx = defaults.iter().find(|b| b.action().name() == name).and_then(|b| b.predicate());
        for k in keys {
            match KeyBinding::load(k, action.boxed_clone(), ctx.clone(), false, None, &DummyKeyboardMapper) {
                Ok(b) => out.push(b),
                Err(e) => errors.push(format!("{name}: '{k}' {e}")),
            }
        }
    }
    cx.clear_key_bindings();
    cx.bind_keys(out);
    errors
}

/// Current shortcuts of an action, for display ("Ctrl+Shift+A").
pub fn shortcuts(cx: &App, name: &str) -> Vec<String> {
    let km = cx.key_bindings();
    let km = km.borrow();
    let mut out: Vec<String> = Vec::new();
    for b in km.bindings().filter(|b| b.action().name() == name) {
        let s = b.keystrokes().iter().map(|k| pretty(&k.unparse())).collect::<Vec<_>>().join(" ");
        if !out.contains(&s) {
            out.push(s);
        }
    }
    out
}

/// "ctrl-shift-a" → "Ctrl+Shift+A".
pub fn pretty(k: &str) -> String {
    k.split('-')
        .map(|p| match p {
            "ctrl" => "Ctrl".to_string(),
            "shift" => "Shift".to_string(),
            "alt" => "Alt".to_string(),
            "cmd" | "super" | "platform" => "Super".to_string(),
            "" => "-".to_string(),
            p if p.chars().count() == 1 => p.to_uppercase(),
            p => {
                let mut c = p.chars();
                c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
            }
        })
        .collect::<Vec<_>>()
        .join("+")
}

#[cfg(test)]
mod tests {
    use super::pretty;

    #[test]
    fn pretty_keys() {
        assert_eq!(pretty("ctrl-shift-a"), "Ctrl+Shift+A");
        assert_eq!(pretty("shift-f10"), "Shift+F10");
        assert_eq!(pretty("alt-enter"), "Alt+Enter");
    }
}
