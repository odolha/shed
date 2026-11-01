//! The split tree written as text: `0` for one pane, or
//! `right 0.6 (0) (down 0.5 (1) (2))`, where numbers are positions in the
//! tab's pane list.

use shed_model::{PaneId, PaneNode, SplitDir};

pub fn format_layout(node: &PaneNode, order: &[PaneId]) -> String {
    match node {
        PaneNode::Leaf(id) => order.iter().position(|p| p == id).unwrap_or(0).to_string(),
        PaneNode::Split { dir, ratio, first, second } => {
            let dir = match dir {
                SplitDir::Right => "right",
                SplitDir::Down => "down",
            };
            let ratio = (ratio * 1000.0).round() / 1000.0;
            format!("{dir} {ratio} ({}) ({})", format_layout(first, order), format_layout(second, order))
        }
    }
}

pub fn parse_layout(text: &str, panes: &[PaneId]) -> Result<PaneNode, String> {
    let mut p = Parser { s: text.as_bytes(), i: 0, panes, used: vec![false; panes.len()] };
    let node = p.node()?;
    p.space();
    if p.i != p.s.len() {
        return Err(format!("unexpected {:?}", &text[p.i..]));
    }
    if let Some(missing) = p.used.iter().position(|u| !u) {
        return Err(format!("pane {missing} is not in the layout"));
    }
    Ok(node)
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
    panes: &'a [PaneId],
    used: Vec<bool>,
}

impl Parser<'_> {
    fn space(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }

    fn word(&mut self) -> &str {
        self.space();
        let start = self.i;
        while self.i < self.s.len() && !self.s[self.i].is_ascii_whitespace() && !b"()".contains(&self.s[self.i]) {
            self.i += 1;
        }
        std::str::from_utf8(&self.s[start..self.i]).unwrap_or("")
    }

    fn expect(&mut self, c: u8) -> Result<(), String> {
        self.space();
        if self.s.get(self.i) == Some(&c) {
            self.i += 1;
            Ok(())
        } else {
            Err(format!("expected '{}'", c as char))
        }
    }

    fn node(&mut self) -> Result<PaneNode, String> {
        let word = self.word().to_string();
        let dir = match word.as_str() {
            "right" => SplitDir::Right,
            "down" => SplitDir::Down,
            n => {
                let index: usize = n.parse().map_err(|_| format!("expected a pane number or right/down, got {n:?}"))?;
                let id = *self.panes.get(index).ok_or_else(|| format!("no pane {index}"))?;
                if std::mem::replace(&mut self.used[index], true) {
                    return Err(format!("pane {index} appears twice"));
                }
                return Ok(PaneNode::Leaf(id));
            }
        };
        let ratio: f32 = self.word().parse().map_err(|_| "expected a ratio".to_string())?;
        self.expect(b'(')?;
        let first = self.node()?;
        self.expect(b')')?;
        self.expect(b'(')?;
        let second = self.node()?;
        self.expect(b')')?;
        Ok(PaneNode::Split { dir, ratio: ratio.clamp(0.05, 0.95), first: Box::new(first), second: Box::new(second) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let ids: Vec<PaneId> = (0..3).map(|_| PaneId::new()).collect();
        let mut node = PaneNode::Leaf(ids[0]);
        node.split(ids[0], ids[1], SplitDir::Right);
        node.split(ids[1], ids[2], SplitDir::Down);
        node.set_ratio(&[], 0.6);
        let text = format_layout(&node, &ids);
        assert_eq!(text, "right 0.6 (0) (down 0.5 (1) (2))");
        assert_eq!(parse_layout(&text, &ids).unwrap(), node);
        assert_eq!(parse_layout(" 0 ", &ids[..1]).unwrap(), PaneNode::Leaf(ids[0]));
    }

    #[test]
    fn bad_layouts_are_explained() {
        let ids: Vec<PaneId> = (0..2).map(|_| PaneId::new()).collect();
        assert!(parse_layout("right 0.5 (0) (0)", &ids).unwrap_err().contains("twice"));
        assert!(parse_layout("0", &ids).unwrap_err().contains("pane 1"));
        assert!(parse_layout("left 0.5 (0) (1)", &ids).is_err());
        assert!(parse_layout("right 0.5 (0) (1) extra", &ids).is_err());
    }
}
