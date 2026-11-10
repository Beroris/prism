//! The annoation document: shapes in image space, plus undo and redo.
//!
//! Shapes live in the coordinates of the whole capture, not of the selection, so moving or
//! resizing the selection after drawing never moves what was drawn.

use crate::color::Rgba;
use crate::geom::{FRect, Pt, dist_to_segment};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Tool {
    /// Pick, move and delete shapes.
    Select,
    #[default]
    Rect,
    Ellipse,
    Line,
    Arrow,
    /// Freehand drawing. Called "pen" in older settings files.
    #[serde(alias = "pen")]
    Draw,
    Highlighter,
    Text,
    Counter,
    Blur,
    Pixelate,
}

impl Tool {
    /// Every drawing tool in toolbar order.
    pub const ALL: [Tool; 11] = [
        Tool::Select,
        Tool::Rect,
        Tool::Ellipse,
        Tool::Line,
        Tool::Arrow,
        Tool::Draw,
        Tool::Highlighter,
        Tool::Text,
        Tool::Counter,
        Tool::Blur,
        Tool::Pixelate,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Tool::Select => "Select and move",
            Tool::Rect => "Rectangle",
            Tool::Ellipse => "Ellipse",
            Tool::Line => "Line",
            Tool::Arrow => "Arrow",
            Tool::Draw => "Draw",
            Tool::Highlighter => "Highlighter",
            Tool::Text => "Text",
            Tool::Counter => "Numbered step",
            Tool::Blur => "Blur",
            Tool::Pixelate => "Pixelate",
        }
    }

    /// The key that picks this tool unless the settings choose another. Draw is D, so it sits
    /// under the left hand while the right holds the mouse.
    pub fn default_key(self) -> char {
        match self {
            Tool::Select => 'V',
            Tool::Rect => 'R',
            Tool::Ellipse => 'E',
            Tool::Line => 'L',
            Tool::Arrow => 'A',
            Tool::Draw => 'D',
            Tool::Highlighter => 'H',
            Tool::Text => 'T',
            Tool::Counter => 'N',
            Tool::Blur => 'B',
            Tool::Pixelate => 'X',
        }
    }

    /// Whether colour applies to this tool. Blur and pixelate have none.
    pub fn uses_color(self) -> bool {
        !matches!(self, Tool::Select | Tool::Blur | Tool::Pixelate)
    }
}

/// Letters the editor uses for something other than picking a tool: C copies the colour under
/// the pointer, F takes the whole monitor and O copies text.
pub const RESERVED_KEYS: [char; 3] = ['C', 'F', 'O'];

/// Which letter picks each tool. Only the changes from `Tool::default_key` are kept, so the
/// settings file stays short and a new default reaches everyone who hasn't changed that tool.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ToolKeys(BTreeMap<Tool, char>);

impl ToolKeys {
    pub fn key(&self, tool: Tool) -> char {
        self.0.get(&tool).copied().unwrap_or(tool.default_key())
    }

    /// The tool a typed letter picks, in either case.
    pub fn tool(&self, c: char) -> Option<Tool> {
        let c = c.to_ascii_uppercase();
        Tool::ALL.into_iter().find(|t| self.key(*t) == c)
    }

    /// Letters a tool can have.
    pub fn usable(c: char) -> bool {
        c.is_ascii_uppercase() && !RESERVED_KEYS.contains(&c)
    }

    /// Give `tool` the letter `key`. A tool that had that letter takes `tool`'s old one, so
    /// every tool keeps a key and no two share one. False if the letter can't be used.
    pub fn set(&mut self, tool: Tool, key: char) -> bool {
        let key = key.to_ascii_uppercase();
        if !Self::usable(key) {
            return false;
        }
        let old = self.key(tool);
        if let Some(other) = self.tool(key).filter(|other| *other != tool) {
            self.0.insert(other, old);
        }
        self.0.insert(tool, key);
        self.0.retain(|tool, key| *key != tool.default_key());
        true
    }

    /// Repair keys edited by hand: lower case is read as upper case, and if any letter can't
    /// be used or two tools share one, every tool goes back to its default.
    pub fn sanitize(&mut self) {
        for key in self.0.values_mut() {
            *key = key.to_ascii_uppercase();
        }
        let mut keys: Vec<char> = Tool::ALL.iter().map(|t| self.key(*t)).collect();
        let all_usable = keys.iter().all(|k| Self::usable(*k));
        keys.sort_unstable();
        keys.dedup();
        if !all_usable || keys.len() != Tool::ALL.len() {
            self.0.clear();
        }
        self.0.retain(|tool, key| *key != tool.default_key());
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Style {
    pub color: Rgba,
    /// Stroke width in image pixels. Text size, counter size and blur strength derive from it.
    pub width: f32,
    /// Filled rectangle or ellipse instead of an outline.
    pub fill: bool,
}

impl Default for Style {
    fn default() -> Self {
        Style { color: Rgba::RED, width: 4.0, fill: false }
    }
}

/// Stroke widths the size control steps through.
pub const WIDTHS: [f32; 6] = [2.0, 3.0, 4.0, 6.0, 9.0, 14.0];

pub fn next_width(w: f32, up: bool) -> f32 {
    if up {
        WIDTHS.iter().copied().find(|&x| x > w + 0.01).unwrap_or(WIDTHS[WIDTHS.len() - 1])
    } else {
        WIDTHS.iter().rev().copied().find(|&x| x < w - 0.01).unwrap_or(WIDTHS[0])
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Rect { a: Pt, b: Pt, style: Style },
    Ellipse { a: Pt, b: Pt, style: Style },
    Line { a: Pt, b: Pt, style: Style },
    Arrow { a: Pt, b: Pt, style: Style },
    Pen { pts: Vec<Pt>, style: Style },
    Highlighter { pts: Vec<Pt>, style: Style },
    /// A highlighter stroke snapped to what was under it: one box per line of text.
    Highlight { rects: Vec<FRect>, style: Style },
    /// `w`/`h` are the measured size of the rendered text, kept for hit testing.
    Text { pos: Pt, text: String, style: Style, w: f32, h: f32 },
    Counter { center: Pt, n: u32, style: Style },
    Blur { a: Pt, b: Pt, strength: f32 },
    Pixelate { a: Pt, b: Pt, block: f32 },
}

/// Text size in pixels for a stroke width.
pub fn text_px(width: f32) -> f32 {
    (12.0 + width * 3.0).round()
}

/// Counter circle radius for a stroke width.
pub fn counter_radius(width: f32) -> f32 {
    (9.0 + width * 2.0).round()
}

/// Arrow head length for a stroke width.
pub fn arrow_head(width: f32) -> f32 {
    (width * 3.5 + 10.0).max(12.0)
}

/// Highlighter stroke width for a stroke width.
pub fn highlighter_width(width: f32) -> f32 {
    (width * 4.0).max(12.0)
}

impl Shape {
    pub fn style(&self) -> Option<&Style> {
        match self {
            Shape::Rect { style, .. }
            | Shape::Ellipse { style, .. }
            | Shape::Line { style, .. }
            | Shape::Arrow { style, .. }
            | Shape::Pen { style, .. }
            | Shape::Highlighter { style, .. }
            | Shape::Highlight { style, .. }
            | Shape::Text { style, .. }
            | Shape::Counter { style, .. } => Some(style),
            Shape::Blur { .. } | Shape::Pixelate { .. } => None,
        }
    }

    /// Bounds of everything the shape paints, including stroke width and arrow heads.
    pub fn bounds(&self) -> FRect {
        match self {
            Shape::Rect { a, b, style } | Shape::Ellipse { a, b, style } | Shape::Line { a, b, style } => {
                FRect::spanning(*a, *b).inflate(style.width / 2.0 + 2.0)
            }
            Shape::Arrow { a, b, style } => FRect::spanning(*a, *b).inflate(arrow_head(style.width) + 2.0),
            Shape::Pen { pts, style } => FRect::of_points(pts).inflate(style.width / 2.0 + 2.0),
            Shape::Highlighter { pts, style } => FRect::of_points(pts).inflate(highlighter_width(style.width) / 2.0 + 2.0),
            Shape::Highlight { rects, .. } => {
                rects.iter().skip(1).fold(rects.first().copied().unwrap_or_default(), |a, r| a.union(r)).inflate(2.0)
            }
            Shape::Text { pos, w, h, .. } => FRect { l: pos.x - 2.0, t: pos.y - 2.0, r: pos.x + w + 2.0, b: pos.y + h + 2.0 },
            Shape::Counter { center, style, .. } => {
                let r = counter_radius(style.width) + 2.0;
                FRect { l: center.x - r, t: center.y - r, r: center.x + r, b: center.y + r }
            }
            Shape::Blur { a, b, .. } | Shape::Pixelate { a, b, .. } => FRect::spanning(*a, *b),
        }
    }

    /// Whether a click at `p` picks this shape. `tol` is the slack in image pixels.
    pub fn hit(&self, p: Pt, tol: f32) -> bool {
        match self {
            Shape::Rect { a, b, style } => {
                let r = FRect::spanning(*a, *b);
                if style.fill {
                    return r.inflate(tol).contains(p);
                }
                let edge = style.width / 2.0 + tol;
                r.inflate(edge).contains(p) && !r.inflate(-edge).contains(p)
            }
            Shape::Ellipse { a, b, style } => {
                let r = FRect::spanning(*a, *b);
                let (cx, cy) = ((r.l + r.r) / 2.0, (r.t + r.b) / 2.0);
                let (rx, ry) = ((r.w() / 2.0).max(1.0), (r.h() / 2.0).max(1.0));
                // Normalised radius; 1.0 is on the outline. Convert the slack to that scale.
                let d = (((p.x - cx) / rx).powi(2) + ((p.y - cy) / ry).powi(2)).sqrt();
                let slack = (style.width / 2.0 + tol) / rx.min(ry);
                if style.fill { d <= 1.0 + slack } else { (d - 1.0).abs() <= slack }
            }
            Shape::Line { a, b, style } => dist_to_segment(p, *a, *b) <= style.width / 2.0 + tol,
            Shape::Arrow { a, b, style } => dist_to_segment(p, *a, *b) <= style.width / 2.0 + tol + 2.0,
            Shape::Pen { pts, style } => polyline_hit(pts, p, style.width / 2.0 + tol),
            Shape::Highlighter { pts, style } => polyline_hit(pts, p, highlighter_width(style.width) / 2.0 + tol),
            Shape::Highlight { rects, .. } => rects.iter().any(|r| r.inflate(tol).contains(p)),
            Shape::Text { .. } | Shape::Counter { .. } | Shape::Blur { .. } | Shape::Pixelate { .. } => {
                self.bounds().inflate(tol).contains(p)
            }
        }
    }

    pub fn translate(&mut self, dx: f32, dy: f32) {
        match self {
            Shape::Rect { a, b, .. }
            | Shape::Ellipse { a, b, .. }
            | Shape::Line { a, b, .. }
            | Shape::Arrow { a, b, .. }
            | Shape::Blur { a, b, .. }
            | Shape::Pixelate { a, b, .. } => {
                *a = a.add(dx, dy);
                *b = b.add(dx, dy);
            }
            Shape::Pen { pts, .. } | Shape::Highlighter { pts, .. } => pts.iter_mut().for_each(|p| *p = p.add(dx, dy)),
            Shape::Highlight { rects, .. } => rects.iter_mut().for_each(|r| *r = r.offset(dx, dy)),
            Shape::Text { pos, .. } => *pos = pos.add(dx, dy),
            Shape::Counter { center, .. } => *center = center.add(dx, dy),
        }
    }

    /// Too small to keep: a click without a drag on a tool that needs one.
    pub fn is_degenerate(&self) -> bool {
        match self {
            Shape::Rect { a, b, .. } | Shape::Ellipse { a, b, .. } | Shape::Blur { a, b, .. } | Shape::Pixelate { a, b, .. } => {
                (a.x - b.x).abs() < 2.0 || (a.y - b.y).abs() < 2.0
            }
            Shape::Line { a, b, .. } | Shape::Arrow { a, b, .. } => a.dist(*b) < 3.0,
            Shape::Pen { pts, .. } | Shape::Highlighter { pts, .. } => pts.is_empty(),
            Shape::Highlight { rects, .. } => rects.is_empty(),
            Shape::Text { text, .. } => text.trim().is_empty(),
            Shape::Counter { .. } => false,
        }
    }
}

fn polyline_hit(pts: &[Pt], p: Pt, r: f32) -> bool {
    match pts {
        [] => false,
        [only] => only.dist(p) <= r,
        _ => pts.windows(2).any(|w| dist_to_segment(p, w[0], w[1]) <= r),
    }
}

/// Snap `b` so the line `a`–`b` is horizontal, vertical or at 45°. Used with Shift.
pub fn snap_angle(a: Pt, b: Pt) -> Pt {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len = (dx * dx + dy * dy).sqrt();
    let step = std::f32::consts::FRAC_PI_4;
    let ang = (dy.atan2(dx) / step).round() * step;
    Pt::new(a.x + len * ang.cos(), a.y + len * ang.sin())
}

/// Snap `b` so `a`–`b` spans a square. Used with Shift on rectangles and ellipses.
pub fn snap_square(a: Pt, b: Pt) -> Pt {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let s = dx.abs().max(dy.abs());
    Pt::new(a.x + s * dx.signum(), a.y + s * dy.signum())
}

/// Shapes with undo and redo. Each change stores a snapshot of the shape list, which is simpler
/// to get right than inverse operations. Snapshots share their shapes, so a snapshot costs a
/// pointer per shape however many points a freehand stroke has.
#[derive(Default, Debug)]
pub struct Doc {
    shapes: Vec<Rc<Shape>>,
    undo: Vec<Vec<Rc<Shape>>>,
    redo: Vec<Vec<Rc<Shape>>>,
}

/// More history than anyone will step back through, bounded so memory can't grow without limit.
const MAX_HISTORY: usize = 200;

impl Doc {
    pub fn shapes(&self) -> &[Rc<Shape>] {
        &self.shapes
    }
    pub fn is_empty(&self) -> bool {
        self.shapes.is_empty()
    }
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    fn checkpoint(&mut self) {
        self.undo.push(self.shapes.clone());
        if self.undo.len() > MAX_HISTORY {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn push(&mut self, s: Shape) {
        self.checkpoint();
        self.shapes.push(Rc::new(s));
    }

    pub fn remove(&mut self, i: usize) -> Option<Shape> {
        if i >= self.shapes.len() {
            return None;
        }
        self.checkpoint();
        let removed = self.shapes.remove(i);
        Some(Rc::unwrap_or_clone(removed))
    }

    /// Replace shape `i`, as one undo step. Used when a move finishes.
    pub fn replace(&mut self, i: usize, s: Shape) {
        if i < self.shapes.len() && *self.shapes[i] != s {
            self.checkpoint();
            self.shapes[i] = Rc::new(s);
        }
    }

    pub fn clear(&mut self) {
        if !self.shapes.is_empty() {
            self.checkpoint();
            self.shapes.clear();
        }
    }

    pub fn undo(&mut self) -> bool {
        match self.undo.pop() {
            Some(prev) => {
                self.redo.push(std::mem::replace(&mut self.shapes, prev));
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self) -> bool {
        match self.redo.pop() {
            Some(next) => {
                self.undo.push(std::mem::replace(&mut self.shapes, next));
                true
            }
            None => false,
        }
    }

    /// The topmost shape under `p`.
    pub fn hit(&self, p: Pt, tol: f32) -> Option<usize> {
        self.shapes.iter().rposition(|s| s.hit(p, tol))
    }

    /// The number the next counter gets: one more than the highest so far.
    pub fn next_counter(&self) -> u32 {
        self.shapes.iter().filter_map(|s| if let Shape::Counter { n, .. } = **s { Some(n) } else { None }).max().unwrap_or(0) + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32) -> Shape {
        Shape::Rect { a: Pt::new(x, 0.0), b: Pt::new(x + 10.0, 10.0), style: Style::default() }
    }

    #[test]
    fn undo_redo_round_trip() {
        let mut d = Doc::default();
        d.push(rect(0.0));
        d.push(rect(20.0));
        assert_eq!(d.shapes().len(), 2);
        assert!(d.undo());
        assert_eq!(d.shapes().len(), 1);
        assert!(d.undo());
        assert!(d.is_empty());
        assert!(!d.undo());
        assert!(d.redo());
        assert!(d.redo());
        assert_eq!(d.shapes().len(), 2);
        assert!(!d.redo());
    }

    #[test]
    fn new_edit_clears_redo() {
        let mut d = Doc::default();
        d.push(rect(0.0));
        d.undo();
        d.push(rect(5.0));
        assert!(!d.can_redo());
    }

    #[test]
    fn replace_without_change_adds_no_history() {
        let mut d = Doc::default();
        d.push(rect(0.0));
        d.replace(0, rect(0.0));
        d.undo();
        assert!(d.is_empty());
    }

    #[test]
    fn hit_outline_not_interior() {
        let s = rect(0.0);
        assert!(s.hit(Pt::new(0.0, 5.0), 3.0));
        assert!(!s.hit(Pt::new(5.0, 5.0), 1.0));
        let filled = Shape::Rect { a: Pt::new(0.0, 0.0), b: Pt::new(10.0, 10.0), style: Style { fill: true, ..Style::default() } };
        assert!(filled.hit(Pt::new(5.0, 5.0), 1.0));
    }

    #[test]
    fn topmost_hit_wins() {
        let mut d = Doc::default();
        let big = Shape::Blur { a: Pt::new(0.0, 0.0), b: Pt::new(100.0, 100.0), strength: 8.0 };
        d.push(big);
        d.push(rect(0.0));
        assert_eq!(d.hit(Pt::new(0.0, 5.0), 3.0), Some(1));
        assert_eq!(d.hit(Pt::new(50.0, 50.0), 3.0), Some(0));
        assert_eq!(d.hit(Pt::new(500.0, 50.0), 3.0), None);
    }

    #[test]
    fn counters_number_from_highest() {
        let mut d = Doc::default();
        let c = |n| Shape::Counter { center: Pt::default(), n, style: Style::default() };
        assert_eq!(d.next_counter(), 1);
        d.push(c(1));
        d.push(c(4));
        assert_eq!(d.next_counter(), 5);
    }

    #[test]
    fn snapping() {
        let b = snap_angle(Pt::new(0.0, 0.0), Pt::new(10.0, 1.0));
        assert!((b.y).abs() < 1e-4 && (b.x - 10.0499).abs() < 1e-2);
        let s = snap_square(Pt::new(0.0, 0.0), Pt::new(10.0, -4.0));
        assert_eq!(s, Pt::new(10.0, -10.0));
    }

    #[test]
    fn width_steps_clamp() {
        assert_eq!(next_width(4.0, true), 6.0);
        assert_eq!(next_width(14.0, true), 14.0);
        assert_eq!(next_width(2.0, false), 2.0);
        assert_eq!(next_width(5.0, false), 4.0);
    }

    #[test]
    fn tool_keys_are_unique() {
        let keys = ToolKeys::default();
        let mut letters: Vec<char> = Tool::ALL.iter().map(|t| keys.key(*t)).collect();
        assert!(letters.iter().all(|k| ToolKeys::usable(*k)));
        letters.sort();
        letters.dedup();
        assert_eq!(letters.len(), Tool::ALL.len());
        assert_eq!(keys.tool('a'), Some(Tool::Arrow));
        assert_eq!(keys.tool('d'), Some(Tool::Draw));
        assert_eq!(keys.tool('p'), None);
    }

    #[test]
    fn choosing_a_taken_letter_swaps_the_two_tools() {
        let mut keys = ToolKeys::default();
        assert!(keys.set(Tool::Rect, 'd'));
        assert_eq!(keys.key(Tool::Rect), 'D');
        assert_eq!(keys.key(Tool::Draw), 'R');
        assert!(!keys.set(Tool::Rect, 'C'), "C copies the colour");
        assert!(!keys.set(Tool::Rect, '1'));
        // Swapping back leaves nothing stored.
        assert!(keys.set(Tool::Rect, 'R'));
        assert_eq!(keys, ToolKeys::default());
    }

    #[test]
    fn hand_edited_keys_are_repaired() {
        let mut keys: ToolKeys = serde_json::from_str(r#"{"draw": "p"}"#).unwrap();
        keys.sanitize();
        assert_eq!(keys.key(Tool::Draw), 'P');
        let mut clash: ToolKeys = serde_json::from_str(r#"{"draw": "R"}"#).unwrap();
        clash.sanitize();
        assert_eq!(clash, ToolKeys::default());
        let mut reserved: ToolKeys = serde_json::from_str(r#"{"pen": "O"}"#).unwrap();
        reserved.sanitize();
        assert_eq!(reserved, ToolKeys::default());
        assert_eq!(serde_json::to_string(&{ let mut k = ToolKeys::default(); k.set(Tool::Blur, 'Q'); k }).unwrap(), r#"{"blur":"Q"}"#);
    }
}
