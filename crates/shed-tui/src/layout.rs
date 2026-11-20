//! A tab's split tree laid out in terminal cells, as tmux does: each split
//! gives one cell of its width (or height) to a dividing line, and the rest
//! to its two sides by the split's ratio.

use ratatui::layout::Rect;
use shed_model::{PaneId, PaneNode, SplitDir};

/// A dividing line between two sides of a split.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Divider {
    /// One cell wide (a vertical line, between sides by side) or one high.
    pub area: Rect,
    pub vertical: bool,
}

/// The panes' rectangles and the lines between them, within `area`. A pane
/// with no room left gets an empty rectangle.
pub fn cells(node: &PaneNode, area: Rect) -> (Vec<(PaneId, Rect)>, Vec<Divider>) {
    let mut panes = Vec::new();
    let mut dividers = Vec::new();
    place(node, area, &mut panes, &mut dividers);
    (panes, dividers)
}

fn place(node: &PaneNode, area: Rect, panes: &mut Vec<(PaneId, Rect)>, dividers: &mut Vec<Divider>) {
    match node {
        PaneNode::Leaf(id) => panes.push((*id, area)),
        PaneNode::Split { dir, ratio, first, second } => {
            let vertical = *dir == SplitDir::Right;
            let total = if vertical { area.width } else { area.height };
            if total < 3 {
                // No room for both: the first side takes it all.
                place(first, area, panes, dividers);
                place(second, Rect { width: 0, height: 0, ..area }, panes, dividers);
                return;
            }
            let room = total - 1;
            let a = ((room as f32 * ratio.clamp(0.0, 1.0)).round() as u16).clamp(1, room - 1);
            let b = room - a;
            let (one, line, two) = if vertical {
                (
                    Rect { width: a, ..area },
                    Rect { x: area.x + a, width: 1, ..area },
                    Rect { x: area.x + a + 1, width: b, ..area },
                )
            } else {
                (
                    Rect { height: a, ..area },
                    Rect { y: area.y + a, height: 1, ..area },
                    Rect { y: area.y + a + 1, height: b, ..area },
                )
            };
            place(first, one, panes, dividers);
            dividers.push(Divider { area: line, vertical });
            place(second, two, panes, dividers);
        }
    }
}

/// Whether the cell at (x, y) touches `r` from outside (sharing an edge).
pub fn touches(r: Rect, x: u16, y: u16) -> bool {
    let inside_x = x >= r.x && x < r.x + r.width;
    let inside_y = y >= r.y && y < r.y + r.height;
    (inside_y && (x + 1 == r.x || x == r.x + r.width)) || (inside_x && (y + 1 == r.y || y == r.y + r.height))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(dir: SplitDir, ratio: f32, a: PaneNode, b: PaneNode) -> PaneNode {
        PaneNode::Split { dir, ratio, first: Box::new(a), second: Box::new(b) }
    }

    #[test]
    fn one_pane_takes_the_whole_area() {
        let p = PaneId::new();
        let area = Rect::new(0, 1, 80, 22);
        let (panes, dividers) = cells(&PaneNode::Leaf(p), area);
        assert_eq!(panes, vec![(p, area)]);
        assert!(dividers.is_empty());
    }

    #[test]
    fn splits_share_the_area_around_a_line() {
        let (a, b, c) = (PaneId::new(), PaneId::new(), PaneId::new());
        // a | (b over c), half and half.
        let tree = split(SplitDir::Right, 0.5, PaneNode::Leaf(a), split(SplitDir::Down, 0.5, PaneNode::Leaf(b), PaneNode::Leaf(c)));
        let (panes, dividers) = cells(&tree, Rect::new(0, 0, 81, 21));
        assert_eq!(panes[0], (a, Rect::new(0, 0, 40, 21)));
        assert_eq!(panes[1], (b, Rect::new(41, 0, 40, 10)));
        assert_eq!(panes[2], (c, Rect::new(41, 11, 40, 10)));
        assert_eq!(dividers[0], Divider { area: Rect::new(40, 0, 1, 21), vertical: true });
        assert_eq!(dividers[1], Divider { area: Rect::new(41, 10, 40, 1), vertical: false });
        // Every cell is a pane's or a line's, once.
        let mut covered = 0u32;
        for (_, r) in &panes {
            covered += r.area();
        }
        for d in &dividers {
            covered += d.area.area();
        }
        assert_eq!(covered, 81 * 21);
    }

    #[test]
    fn ratios_are_kept_and_sides_never_vanish() {
        let (a, b) = (PaneId::new(), PaneId::new());
        let (panes, _) = cells(&split(SplitDir::Right, 0.25, PaneNode::Leaf(a), PaneNode::Leaf(b)), Rect::new(0, 0, 101, 10));
        assert_eq!((panes[0].1.width, panes[1].1.width), (25, 75));
        let (panes, _) = cells(&split(SplitDir::Right, 0.0, PaneNode::Leaf(a), PaneNode::Leaf(b)), Rect::new(0, 0, 10, 10));
        assert_eq!((panes[0].1.width, panes[1].1.width), (1, 8));
        // Too small for a line: the first side has it all.
        let (panes, dividers) = cells(&split(SplitDir::Down, 0.5, PaneNode::Leaf(a), PaneNode::Leaf(b)), Rect::new(0, 0, 10, 2));
        assert_eq!((panes[0].1.height, panes[1].1.area()), (2, 0));
        assert!(dividers.is_empty());
    }

    #[test]
    fn lines_next_to_a_pane_touch_it() {
        let r = Rect::new(10, 5, 4, 3);
        assert!(touches(r, 9, 6));
        assert!(touches(r, 14, 5));
        assert!(touches(r, 11, 4));
        assert!(touches(r, 13, 8));
        assert!(!touches(r, 9, 4));
        assert!(!touches(r, 11, 6));
    }
}
