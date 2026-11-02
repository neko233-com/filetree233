//! Bounded, hierarchical rectangular treemap layout.
use crate::tree::Snapshot;
use eframe::egui::{Rect, pos2};

/// A file, directory, or aggregated remainder in the treemap.
#[derive(Clone, Copy)]
pub struct Tile {
    pub id: u32,
    pub rect: Rect,
    pub remainder: bool,
    pub logical: u64,
}
pub fn layout(tree: &Snapshot, root: u32, area: Rect, max_tiles: usize) -> Vec<Tile> {
    let mut output = Vec::new();
    if max_tiles == 0 || !area.is_positive() {
        return output;
    }
    let mut pending = vec![(root, area, false, tree.nodes[root as usize].logical)];
    while let Some((id, rect, remainder, logical)) = pending.pop() {
        let available = max_tiles.saturating_sub(output.len() + pending.len());
        if remainder || rect.width() < 12.0 || rect.height() < 12.0 || available < 2 {
            output.push(Tile {
                id,
                rect,
                remainder,
                logical,
            });
            continue;
        }
        let node = &tree.nodes[id as usize];
        let mut items = Vec::with_capacity(available.min(256));
        let mut remaining = 0u64;
        if node.is_dir {
            for child in tree.child_ids(id) {
                let size = tree.nodes[child as usize].logical;
                if size == 0 {
                    continue;
                }
                if remaining > 0 {
                    remaining = remaining.saturating_add(size);
                } else if items.len() < available {
                    items.push((child, size));
                } else {
                    remaining = items.pop().map_or(0, |x| x.1).saturating_add(size);
                }
            }
        }
        if items.is_empty() {
            output.push(Tile {
                id,
                rect,
                remainder: false,
                logical,
            });
            continue;
        }
        let grouped = remaining > 0;
        if grouped {
            items.push((id, remaining));
        }
        let mut split = Vec::with_capacity(items.len());
        partition(&items, rect, &mut split);
        let last = split.len().saturating_sub(1);
        pending.extend(
            split
                .into_iter()
                .enumerate()
                .rev()
                .map(|(i, (child, rect))| (child, rect, grouped && i == last, items[i].1)),
        );
    }
    output
}
fn partition(items: &[(u32, u64)], rect: Rect, output: &mut Vec<(u32, Rect)>) {
    let mut pending = vec![(0usize, items.len(), rect)];
    while let Some((start, end, rect)) = pending.pop() {
        let slice = &items[start..end];
        if slice.len() == 1 {
            output.push((slice[0].0, rect));
            continue;
        }
        let total: u128 = slice.iter().map(|x| u128::from(x.1)).sum();
        let mut first = 0u128;
        let mut cut = 0;
        for item in &slice[..slice.len() - 1] {
            first += u128::from(item.1);
            cut += 1;
            if first * 2 >= total {
                break;
            }
        }
        let ratio = first as f64 / total as f64;
        let (a, b) = if rect.width() >= rect.height() {
            let x = rect.min.x + rect.width() * ratio as f32;
            (
                Rect::from_min_max(rect.min, pos2(x, rect.max.y)),
                Rect::from_min_max(pos2(x, rect.min.y), rect.max),
            )
        } else {
            let y = rect.min.y + rect.height() * ratio as f32;
            (
                Rect::from_min_max(rect.min, pos2(rect.max.x, y)),
                Rect::from_min_max(pos2(rect.min.x, y), rect.max),
            )
        };
        pending.push((start + cut, end, b));
        pending.push((start, start + cut, a));
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn many_entries_are_grouped_without_exceeding_tile_budget() {
        let node = |name: &str, parent: u32, first: u32, next: u32, is_dir: bool, size: u64| {
            crate::tree::Node {
                name: name.into(),
                parent,
                first_child: first,
                next_sibling: next,
                logical: size,
                allocated: size,
                files: 0,
                directories: 0,
                is_dir,
                error: None,
                complete: true,
            }
        };
        let mut nodes = vec![node("", u32::MAX, 1, u32::MAX, true, 10)];
        for i in 1..=10 {
            nodes.push(node(
                "file",
                0,
                u32::MAX,
                if i < 10 { i + 1 } else { u32::MAX },
                false,
                1,
            ));
        }
        let tree = crate::tree::Snapshot {
            root: "/demo".into(),
            nodes,
        };
        let rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 100.0));
        let tiles = layout(&tree, 0, rect, 3);
        assert_eq!(tiles.len(), 3);
        assert_eq!(tiles.iter().find(|t| t.remainder).unwrap().logical, 8);
        let area: f32 = tiles.iter().map(|t| t.rect.width() * t.rect.height()).sum();
        assert!((area - 10000.0).abs() < 0.2);
    }
    #[test]
    fn partition_preserves_area_and_bounds() {
        let area = Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 60.0));
        let mut parts = Vec::new();
        partition(&[(1, 6), (2, 3), (3, 1)], area, &mut parts);
        assert_eq!(parts.len(), 3);
        let sum: f32 = parts.iter().map(|(_, r)| r.width() * r.height()).sum();
        assert!((sum - 6000.0).abs() < 0.1);
        for (_, r) in &parts {
            assert!(area.contains_rect(*r));
        }
        for i in 0..parts.len() {
            for j in i + 1..parts.len() {
                assert!(!parts[i].1.intersect(parts[j].1).is_positive());
            }
        }
    }
}
