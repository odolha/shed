//! A bench's tabs in groups. A group starts at a tab with a divider before
//! it (the first tab's divider counts for nothing: it always starts one) and
//! goes on to the next such tab. Groups have no names; they are known by
//! their tabs.
//!
//! Rearranging several tabs at once (a group moved, a tab made a group of
//! its own) is worked out here as the bench's whole new order, which a
//! window sends in one `ArrangeTabs`; [`crate::Shed::arrange_tabs`] applies it.

use std::ops::Range;

use crate::{Bench, TabDivider, TabId};

/// A bench's tabs in order, each with the divider before it.
pub type Arrangement = Vec<(TabId, TabDivider)>;

/// The bench's tabs as an [`Arrangement`].
pub fn of(bench: &Bench) -> Arrangement {
    bench.tabs.iter().map(|t| (t.id, t.divider)).collect()
}

/// The groups, as ranges of positions.
pub fn groups(tabs: &[(TabId, TabDivider)]) -> Vec<Range<usize>> {
    let mut out: Vec<Range<usize>> = Vec::new();
    for (i, (_, divider)) in tabs.iter().enumerate() {
        match out.last_mut() {
            Some(last) if *divider == TabDivider::None => last.end = i + 1,
            _ => out.push(i..i + 1),
        }
    }
    out
}

/// Which group a tab is in.
pub fn group_of(tabs: &[(TabId, TabDivider)], tab: TabId) -> Option<usize> {
    let at = tabs.iter().position(|(t, _)| *t == tab)?;
    groups(tabs).iter().position(|g| g.contains(&at))
}

/// Takes `tab` out; a group's divider is on its first tab, so leaving, that
/// tab hands it to the next (as moving a tab does).
fn take(tabs: &mut Arrangement, tab: TabId) -> bool {
    let Some(at) = tabs.iter().position(|(t, _)| *t == tab) else { return false };
    let (_, divider) = tabs.remove(at);
    if divider != TabDivider::None
        && let Some(next) = tabs.get_mut(at)
        && next.1 == TabDivider::None
    {
        next.1 = divider;
    }
    true
}

/// `tab` as a group of its own just before `before` (last, with `None`):
/// set apart by a space (`TabDivider::Space`), or on a row of its own
/// (`TabDivider::Row`: what was there starts the next row).
pub fn alone_before(tabs: &[(TabId, TabDivider)], tab: TabId, before: Option<TabId>, divider: TabDivider) -> Arrangement {
    let mut out = tabs.to_vec();
    let at = out.iter().position(|(t, _)| *t == tab);
    // Before itself: before the tab after it.
    let before = match before {
        Some(b) if b == tab => at.and_then(|at| out.get(at + 1)).map(|(t, _)| *t),
        b => b,
    };
    if !take(&mut out, tab) {
        return out;
    }
    let index = before.and_then(|b| out.iter().position(|(t, _)| *t == b)).unwrap_or(out.len());
    out.insert(index, (tab, divider));
    if let Some(next) = out.get_mut(index + 1) {
        if divider == TabDivider::Row {
            next.1 = TabDivider::Row;
        } else if next.1 == TabDivider::None {
            next.1 = TabDivider::Space;
        }
    }
    out
}

/// Group `group` swapped with the one before it (`right`: after it), each
/// keeping its tabs' order and its divider. `None` if there is no such group.
pub fn move_group(tabs: &[(TabId, TabDivider)], group: usize, right: bool) -> Option<Arrangement> {
    let gs = groups(tabs);
    let (a, b) = if right { (group, group + 1) } else { (group.checked_sub(1)?, group) };
    if b >= gs.len() {
        return None;
    }
    let mut out: Arrangement = Vec::with_capacity(tabs.len());
    for (i, g) in gs.iter().enumerate() {
        let g = if i == a { &gs[b] } else if i == b { &gs[a] } else { g };
        // The group that was first may have no divider of its own: it gets a space.
        if !out.is_empty() && tabs[g.start].1 == TabDivider::None {
            out.push((tabs[g.start].0, TabDivider::Space));
            out.extend_from_slice(&tabs[g.start + 1..g.end]);
        } else {
            out.extend_from_slice(&tabs[g.clone()]);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use TabDivider::{None as N, Row as R, Space as S};

    fn ids(n: usize) -> Vec<TabId> {
        (0..n).map(|_| TabId::new()).collect()
    }

    #[test]
    fn groups_start_at_dividers() {
        let t = ids(5);
        let tabs = vec![(t[0], R), (t[1], N), (t[2], S), (t[3], R), (t[4], N)];
        assert_eq!(groups(&tabs), [0..2, 2..3, 3..5]);
        assert_eq!(group_of(&tabs, t[4]), Some(2));
    }

    #[test]
    fn a_tab_alone_before_a_group_or_a_row() {
        let t = ids(5);
        // [0 1] [2 3] [4]
        let tabs = vec![(t[0], N), (t[1], N), (t[2], S), (t[3], N), (t[4], S)];
        // 1 between the first two groups: a group of its own.
        assert_eq!(alone_before(&tabs, t[1], Some(t[2]), S), [(t[0], N), (t[1], S), (t[2], S), (t[3], N), (t[4], S)]);
        // 2 (its group's first) on a row of its own before 4: 3 keeps the group going.
        assert_eq!(alone_before(&tabs, t[2], Some(t[4]), R), [(t[0], N), (t[1], N), (t[3], S), (t[2], R), (t[4], R)]);
        // Before a tab inside a group: what follows becomes a group too.
        assert_eq!(alone_before(&tabs, t[0], Some(t[3]), S), [(t[1], N), (t[2], S), (t[0], S), (t[3], S), (t[4], S)]);
        // A row at the very start.
        assert_eq!(alone_before(&tabs, t[3], Some(t[0]), R), [(t[3], R), (t[0], R), (t[1], N), (t[2], S), (t[4], S)]);
        // Last, as a group of its own.
        assert_eq!(alone_before(&tabs, t[0], None, S), [(t[1], N), (t[2], S), (t[3], N), (t[4], S), (t[0], S)]);
        // Before itself: it parts from the tabs after it.
        assert_eq!(alone_before(&tabs, t[2], Some(t[2]), S), [(t[0], N), (t[1], N), (t[2], S), (t[3], S), (t[4], S)]);
    }

    #[test]
    fn groups_move_with_their_dividers() {
        let t = ids(5);
        // [0 1] [2] [3 4 on a row]
        let tabs = vec![(t[0], N), (t[1], N), (t[2], S), (t[3], R), (t[4], N)];
        assert_eq!(move_group(&tabs, 2, false).unwrap(), [(t[0], N), (t[1], N), (t[3], R), (t[4], N), (t[2], S)]);
        // The first group, moved right, gets a space of its own.
        assert_eq!(move_group(&tabs, 0, true).unwrap(), [(t[2], S), (t[0], S), (t[1], N), (t[3], R), (t[4], N)]);
        assert_eq!(move_group(&tabs, 1, false), move_group(&tabs, 0, true));
        assert!(move_group(&tabs, 0, false).is_none());
        assert!(move_group(&tabs, 2, true).is_none());
    }
}
