//! The order of the tool strip's icons (Presets, Edit, Crop, Remove, Masking, Portrait, Red Eye,
//! Versions, Activity, Keywords, Info), and the drag that changes it. Portrait is the fork's
//! (`fork/retouch-foundation.md`).
//!
//! The strip has eleven places: two above the first divider, five between the dividers, two below
//! the second and two pinned to the bottom (the About button stays after the ninth). An icon
//! moves from place to place; the dividers stay where they are. The order is saved with the UI
//! state (`ui.json`, key `toolStrip`, a list of icon ids); a state saved before it existed loads
//! as the usual order, and what a file or an agent says wrongly is repaired: unknown and repeated
//! ids are dropped and icons not named come last in their usual order.
//!
//! Agents use `view.toolStrip {order?}` (no params reads it), `view.toolStripMove {tool,
//! direction | before}` and `view.toolStripReset`.

use egui::{Rect, pos2, vec2};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::icons::Icon;
use crate::theme::Tokens;
use crate::widgets::register;

#[cfg(test)]
mod tests;

/// One icon of the strip.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tool {
    Presets,
    Edit,
    Crop,
    Remove,
    Masking,
    /// Fork: portrait retouching (`fork/retouch-foundation.md`).
    Portrait,
    RedEye,
    Versions,
    Activity,
    Keywords,
    Info,
}

impl Tool {
    /// Every icon, in the order the strip starts with.
    pub const ALL: [Tool; 11] = [
        Tool::Presets,
        Tool::Edit,
        Tool::Crop,
        Tool::Remove,
        Tool::Masking,
        Tool::Portrait,
        Tool::RedEye,
        Tool::Versions,
        Tool::Activity,
        Tool::Keywords,
        Tool::Info,
    ];

    /// The name commands and the saved state use; also the icon's widget (`icon:<id>`).
    pub const fn id(self) -> &'static str {
        match self {
            Tool::Presets => "presets",
            Tool::Edit => "edit",
            Tool::Crop => "crop",
            Tool::Remove => "remove",
            Tool::Masking => "masking",
            Tool::RedEye => "redeye",
            Tool::Portrait => "portrait",
            Tool::Versions => "versions",
            Tool::Activity => "activity",
            Tool::Keywords => "keywords",
            Tool::Info => "info",
        }
    }

    /// The name shown while it is dragged, in English (translated where it is shown).
    pub const fn title(self) -> &'static str {
        match self {
            Tool::Presets => "Presets",
            Tool::Edit => "Edit",
            Tool::Crop => "Crop & Rotate",
            Tool::Remove => "Remove",
            Tool::Masking => "Masking",
            Tool::RedEye => "Red Eye",
            Tool::Portrait => "Portrait",
            Tool::Versions => "Versions",
            Tool::Activity => "History & Activity",
            Tool::Keywords => "Keywords",
            Tool::Info => "Info",
        }
    }

    /// The icon the strip draws for it.
    pub const fn icon(self) -> Icon {
        match self {
            Tool::Presets => Icon::Presets,
            Tool::Edit => Icon::Sliders,
            Tool::Crop => Icon::Crop,
            Tool::Remove => Icon::Eraser,
            Tool::Masking => Icon::Mask,
            Tool::RedEye => Icon::Eye,
            Tool::Portrait => Icon::Subject,
            Tool::Versions => Icon::Versions,
            Tool::Activity => Icon::Activity,
            Tool::Keywords => Icon::Tag,
            Tool::Info => Icon::Info,
        }
    }

    pub fn from_id(id: &str) -> Option<Tool> {
        Self::ALL.into_iter().find(|t| t.id() == id)
    }

    /// The ids of every icon, for error messages.
    pub fn ids() -> String {
        Self::ALL.map(Tool::id).join("|")
    }
}

/// The icons top to bottom; every icon is in it exactly once.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "Value", into = "Vec<&'static str>")]
pub struct ToolStripOrder(Vec<Tool>);

impl Default for ToolStripOrder {
    fn default() -> Self {
        Self(Tool::ALL.to_vec())
    }
}

impl From<ToolStripOrder> for Vec<&'static str> {
    fn from(order: ToolStripOrder) -> Self {
        order.0.into_iter().map(Tool::id).collect()
    }
}

/// Lenient on purpose: a state file edited by hand, written by a newer version or damaged still
/// opens.
impl From<Value> for ToolStripOrder {
    fn from(saved: Value) -> Self {
        let entries = saved.as_array().map(Vec::as_slice).unwrap_or_default();
        let mut tools: Vec<Tool> = entries.iter().filter_map(|e| Tool::from_id(e.as_str()?)).collect();
        // fork: an order saved before the Portrait icon gets it after Masking, not last (a bottom place)
        if !tools.contains(&Tool::Portrait)
            && let Some(at) = tools.iter().position(|t| *t == Tool::Masking)
        {
            tools.insert(at + 1, Tool::Portrait);
        }
        Self::from_tools(tools)
    }
}

impl ToolStripOrder {
    /// The order these icons describe, made whole (see the type).
    fn from_tools(tools: impl IntoIterator<Item = Tool>) -> Self {
        let mut all: Vec<Tool> = Vec::with_capacity(Tool::ALL.len());
        for tool in tools.into_iter().chain(Tool::ALL) {
            if !all.contains(&tool) {
                all.push(tool);
            }
        }
        Self(all)
    }

    /// Every icon top to bottom.
    pub fn tools(&self) -> &[Tool] {
        &self.0
    }

    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Put `tool` just above `before`, or last when `before` is `None`. Returns whether the order
    /// changed.
    pub fn move_before(&mut self, tool: Tool, before: Option<Tool>) -> bool {
        if before == Some(tool) {
            return false;
        }
        let was = self.0.clone();
        self.0.retain(|t| *t != tool);
        let at = before.and_then(|b| self.0.iter().position(|t| *t == b)).unwrap_or(self.0.len());
        self.0.insert(at, tool);
        self.0 != was
    }

    /// Move `tool` one place up or down. Returns whether it moved.
    pub fn move_by(&mut self, tool: Tool, up: bool) -> bool {
        let Some(at) = self.0.iter().position(|t| *t == tool) else { return false };
        if up {
            let Some(above) = at.checked_sub(1).and_then(|i| self.0.get(i)).copied() else { return false };
            self.move_before(tool, Some(above))
        } else {
            if at + 1 >= self.0.len() {
                return false;
            }
            self.move_before(tool, self.0.get(at + 2).copied())
        }
    }

    pub fn can_move(&self, tool: Tool, up: bool) -> bool {
        match self.0.iter().position(|t| *t == tool) {
            Some(at) if up => at > 0,
            Some(at) => at + 1 < self.0.len(),
            None => false,
        }
    }

    /// The order an agent asks for, read strictly: a list of icon ids, each at most once (those
    /// left out follow in the usual order).
    pub fn with_request(order: &Value) -> Result<Self, String> {
        let list = order.as_array().ok_or_else(|| format!("view.toolStrip: `order` must be a list of tools ({}), not {order}", Tool::ids()))?;
        let mut out: Vec<Tool> = Vec::new();
        for entry in list {
            let name = entry.as_str().ok_or_else(|| format!("view.toolStrip: `order` has {entry}, which names no tool ({})", Tool::ids()))?;
            let tool = Tool::from_id(name).ok_or_else(|| format!("view.toolStrip: unknown tool `{name}` ({})", Tool::ids()))?;
            if out.contains(&tool) {
                return Err(format!("view.toolStrip: `{name}` is listed twice in `order`"));
            }
            out.push(tool);
        }
        Ok(Self::from_tools(out))
    }

    pub fn to_json(&self) -> Value {
        let order: Vec<&str> = self.0.iter().map(|t| t.id()).collect();
        json!({"order": order, "default": self.is_default()})
    }
}

/// The UI commands of this feature; `None` when `id` is not one of them.
pub fn command(app: &mut LightcraftApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    let tool = |p: &Value| -> Result<Tool, String> {
        let name = p.get("tool").and_then(Value::as_str).ok_or_else(|| format!("{id}: `tool` is needed ({})", Tool::ids()))?;
        Tool::from_id(name).ok_or_else(|| format!("{id}: unknown tool `{name}` ({})", Tool::ids()))
    };
    let order = &mut app.ui.tool_strip;
    let r = match id {
        "view.toolStrip" => match p.get("order").filter(|v| !v.is_null()) {
            Some(v) => ToolStripOrder::with_request(v).map(|next| {
                *order = next;
                order.to_json()
            }),
            None => Ok(order.to_json()),
        },
        "view.toolStripMove" => tool(p).and_then(|t| {
            let moved = match (p.get("direction"), p.get("before")) {
                (Some(d), None) => match d.as_str() {
                    Some("up") => order.move_by(t, true),
                    Some("down") => order.move_by(t, false),
                    _ => return Err(format!("{id}: `direction` must be \"up\" or \"down\", not {d}")),
                },
                (None, Some(Value::Null)) => order.move_before(t, None),
                (None, Some(b)) => match b.as_str().and_then(Tool::from_id) {
                    Some(before) => order.move_before(t, Some(before)),
                    None => return Err(format!("{id}: `before` must be a tool ({}) or null, not {b}", Tool::ids())),
                },
                _ => return Err(format!("{id}: give either `direction` (up|down) or `before` (a tool, or null for last)")),
            };
            let mut out = order.to_json();
            out["moved"] = json!(moved);
            Ok(out)
        }),
        "view.toolStripReset" => {
            *order = ToolStripOrder::default();
            Ok(order.to_json())
        }
        _ => return None,
    };
    Some(r)
}

/// The icons' places on screen this frame, top to bottom, for [`drag`].
pub type Placed = Vec<(Tool, Rect)>;

/// Where an icon dragged to `y` would go: above the first icon whose middle is below `y`, or last
/// (`None`).
pub fn drop_before(placed: &[(Tool, Rect)], y: f32) -> Option<Tool> {
    placed.iter().find(|(_, r)| y < r.center().y).map(|(t, _)| *t)
}

/// The right-click menu items of an icon: Move Up, Move Down, Reset Order. Widgets
/// `toolStripMenu:moveUp`, `:moveDown` and `:reset`.
pub fn menu(app: &mut LightcraftApp, ui: &mut egui::Ui, tool: Tool) {
    let mut run = None;
    for (up, label, widget) in [(true, "Move Up", "toolStripMenu:moveUp"), (false, "Move Down", "toolStripMenu:moveDown")] {
        let item = ui.add_enabled(app.ui.tool_strip.can_move(tool, up), egui::Button::new(crate::i18n::tr(label)));
        register(ui, widget, item.rect);
        if item.clicked() {
            run = Some(("view.toolStripMove", json!({"tool": tool.id(), "direction": if up { "up" } else { "down" }})));
        }
    }
    if let Some(command) = crate::keybinds::context::command_for_widget(&format!("icon:{}", tool.id())) {
        ui.separator();
        crate::keybinds::context::assign_item(ui, command);
    }
    ui.separator();
    let reset = ui.add_enabled(!app.ui.tool_strip.is_default(), egui::Button::new(crate::i18n::tr("Reset Order")));
    register(ui, "toolStripMenu:reset", reset.rect);
    if reset.clicked() {
        run = Some(("view.toolStripReset", json!({})));
    }
    if let Some((id, params)) = run {
        if let Err(e) = app.run(id, params) {
            app.toast(ui.ctx(), e);
        }
        ui.close();
    }
}

/// While an icon is dragged: a line shows where it would go, releasing over the strip moves it
/// there (`view.toolStripMove`), releasing elsewhere or Esc leaves it. `placed` is where the
/// icons were drawn this frame.
pub fn drag(app: &mut LightcraftApp, ui: &mut egui::Ui, placed: &[(Tool, Rect)]) {
    let Some(dragged) = app.ui.dragging_tool else { return };
    let (down, released, pos, esc) =
        ui.input(|i| (i.pointer.primary_down(), i.pointer.primary_released(), i.pointer.latest_pos(), i.key_pressed(egui::Key::Escape)));
    if esc {
        crate::widgets::take_escape(ui.ctx());
        app.ui.dragging_tool = None;
        return;
    }
    if !down && !released {
        app.ui.dragging_tool = None;
        return;
    }
    let (Some(first), Some(last)) = (placed.first(), placed.last()) else {
        app.ui.dragging_tool = None;
        return;
    };
    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    // over the strip, with some room beside it for a hand that drifts
    let strip = ui.clip_rect().expand2(vec2(24.0, 0.0));
    let Some(pos) = pos.filter(|p| strip.contains(*p)) else {
        if released {
            app.ui.dragging_tool = None;
        }
        return;
    };
    let before = drop_before(placed, pos.y);
    if released {
        app.ui.dragging_tool = None;
        let before = before.map_or(Value::Null, |b| json!(b.id()));
        if let Err(e) = app.run("view.toolStripMove", json!({"tool": dragged.id(), "before": before})) {
            app.toast(ui.ctx(), e);
        }
        return;
    }
    let y = match before {
        Some(b) => placed.iter().find(|(t, _)| *t == b).map(|(_, r)| r.top() - 3.0),
        None => Some(last.1.bottom() + 3.0),
    };
    if let Some(y) = y {
        let t = Tokens::get(ui.ctx());
        let line = Rect::from_min_max(pos2(first.1.left() + 4.0, y - 1.0), pos2(first.1.right() - 4.0, y + 1.0));
        ui.painter().rect_filled(line, 1.0, t.accent);
        register(ui, "toolStripDrop", line);
    }
    if let Some(y) = crate::drag_ghost::row_centre(placed.iter().map(|(_, r)| *r), pos.y) {
        crate::drag_ghost::show(ui.ctx(), pos, y, dragged.icon());
    }
}
