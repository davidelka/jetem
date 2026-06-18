//! The pane layout: a binary split tree (tmux/i3 style). Leaves hold pane ids;
//! splits divide their area left/right or top/bottom by a ratio. Pure geometry
//! and tree surgery — no rendering or pane ownership here.

use crate::pane::Rect;

pub type PaneId = usize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitDir {
    LeftRight,
    TopBottom,
}

pub enum Layout {
    Leaf(PaneId),
    Split {
        dir: SplitDir,
        ratio: f32,
        a: Box<Layout>,
        b: Box<Layout>,
    },
}

impl Layout {
    /// Assign a pixel rect to every leaf by recursively dividing `area`, leaving
    /// `gap` pixels between siblings for a visible divider.
    pub fn compute_rects(&self, area: Rect, gap: usize, out: &mut Vec<(PaneId, Rect)>) {
        match self {
            Layout::Leaf(id) => out.push((*id, area)),
            Layout::Split { dir, ratio, a, b } => match dir {
                SplitDir::LeftRight => {
                    let usable = area.w.saturating_sub(gap);
                    let aw = (usable as f32 * ratio) as usize;
                    let bw = usable - aw;
                    let ra = Rect::new(area.x, area.y, aw, area.h);
                    let rb = Rect::new(area.x + aw + gap, area.y, bw, area.h);
                    a.compute_rects(ra, gap, out);
                    b.compute_rects(rb, gap, out);
                }
                SplitDir::TopBottom => {
                    let usable = area.h.saturating_sub(gap);
                    let ah = (usable as f32 * ratio) as usize;
                    let bh = usable - ah;
                    let ra = Rect::new(area.x, area.y, area.w, ah);
                    let rb = Rect::new(area.x, area.y + ah + gap, area.w, bh);
                    a.compute_rects(ra, gap, out);
                    b.compute_rects(rb, gap, out);
                }
            },
        }
    }

    /// Split the leaf holding `target` into `[target | new_id]` along `dir`.
    /// Consumes and returns the (possibly rewritten) tree.
    pub fn split(self, target: PaneId, dir: SplitDir, new_id: PaneId) -> Layout {
        match self {
            Layout::Leaf(id) if id == target => Layout::Split {
                dir,
                ratio: 0.5,
                a: Box::new(Layout::Leaf(id)),
                b: Box::new(Layout::Leaf(new_id)),
            },
            Layout::Leaf(id) => Layout::Leaf(id),
            Layout::Split {
                dir: d,
                ratio,
                a,
                b,
            } => Layout::Split {
                dir: d,
                ratio,
                a: Box::new(a.split(target, dir, new_id)),
                b: Box::new(b.split(target, dir, new_id)),
            },
        }
    }

    /// Remove the leaf holding `target`, collapsing its parent split to the
    /// surviving sibling. Returns `None` if the whole tree was that one leaf.
    pub fn remove(self, target: PaneId) -> Option<Layout> {
        match self {
            Layout::Leaf(id) if id == target => None,
            leaf @ Layout::Leaf(_) => Some(leaf),
            Layout::Split {
                dir,
                ratio,
                a,
                b,
            } => match (a.remove(target), b.remove(target)) {
                (Some(x), None) | (None, Some(x)) => Some(x), // collapse to sibling
                (Some(a), Some(b)) => Some(Layout::Split {
                    dir,
                    ratio,
                    a: Box::new(a),
                    b: Box::new(b),
                }),
                (None, None) => None, // unreachable: ids are unique
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rects(layout: &Layout, w: usize, h: usize) -> Vec<(PaneId, Rect)> {
        let mut out = Vec::new();
        layout.compute_rects(Rect::new(0, 0, w, h), 0, &mut out);
        out
    }

    #[test]
    fn single_leaf_fills_area() {
        let l = Layout::Leaf(7);
        assert_eq!(rects(&l, 100, 50), vec![(7, Rect::new(0, 0, 100, 50))]);
    }

    #[test]
    fn left_right_split_divides_width() {
        let l = Layout::Leaf(0).split(0, SplitDir::LeftRight, 1);
        let r = rects(&l, 100, 50);
        assert_eq!(r[0], (0, Rect::new(0, 0, 50, 50)));
        assert_eq!(r[1], (1, Rect::new(50, 0, 50, 50)));
    }

    #[test]
    fn top_bottom_split_divides_height() {
        let l = Layout::Leaf(0).split(0, SplitDir::TopBottom, 1);
        let r = rects(&l, 100, 80);
        assert_eq!(r[0], (0, Rect::new(0, 0, 100, 40)));
        assert_eq!(r[1], (1, Rect::new(0, 40, 100, 40)));
    }

    #[test]
    fn nested_split_three_panes() {
        // Split 0|1, then split 1 into 1/2 (top/bottom).
        let l = Layout::Leaf(0)
            .split(0, SplitDir::LeftRight, 1)
            .split(1, SplitDir::TopBottom, 2);
        let ids: Vec<PaneId> = rects(&l, 100, 100).into_iter().map(|(id, _)| id).collect();
        assert_eq!(ids, vec![0, 1, 2]);
    }

    #[test]
    fn remove_collapses_to_sibling() {
        let l = Layout::Leaf(0).split(0, SplitDir::LeftRight, 1);
        let l = l.remove(0).unwrap();
        // Only pane 1 remains, now filling the whole area.
        assert_eq!(rects(&l, 100, 50), vec![(1, Rect::new(0, 0, 100, 50))]);
    }

    #[test]
    fn remove_last_leaf_is_none() {
        let l = Layout::Leaf(0);
        assert!(l.remove(0).is_none());
    }

    #[test]
    fn gap_is_left_between_siblings() {
        let l = Layout::Leaf(0).split(0, SplitDir::LeftRight, 1);
        let mut out = Vec::new();
        l.compute_rects(Rect::new(0, 0, 101, 50), 1, &mut out);
        // usable = 100, split 50/50, gap of 1px between.
        assert_eq!(out[0].1, Rect::new(0, 0, 50, 50));
        assert_eq!(out[1].1, Rect::new(51, 0, 50, 50));
    }
}
