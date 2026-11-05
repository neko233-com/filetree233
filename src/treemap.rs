//! Bounded, squarified hierarchical treemap layout (Bruls, Huizing & van Wijk).
use crate::tree::Snapshot;
use eframe::egui::{Rect, pos2};
use std::cmp::Reverse;
use std::collections::BinaryHeap;

/// A file, directory, or aggregated remainder in the treemap.
#[derive(Clone, Copy, Debug)]
pub struct Tile {
    pub id: u32,
    pub rect: Rect,
    pub remainder: bool,
    /// Metric value the tile area is proportional to; `0` for an empty remainder.
    pub weight: u64,
    /// Nesting level below the laid-out root: 0 for its direct children.
    pub depth: usize,
}

/// Tiles below `min_side` are folded into their parent instead of drawn.
const MIN_SIDE: f32 = 12.0;
/// One directory never contributes more than this many children per pass.
const MAX_CHILDREN: usize = 4096;

/// Tiles the subtree under `root` into `area`, drawing at most `depth_limit`
/// nested levels and never more than `max_tiles` rectangles.
pub fn layout(
    tree: &Snapshot,
    root: u32,
    area: Rect,
    max_tiles: usize,
    depth_limit: usize,
    weight: &dyn Fn(&crate::tree::Node) -> u64,
) -> Vec<Tile> {
    let mut output = Vec::new();
    if max_tiles == 0 || !area.is_positive() {
        return output;
    }
    let mut pending = vec![(
        root,
        area,
        false,
        weight(&tree.nodes[root as usize]),
        0usize,
    )];
    while let Some((id, rect, remainder, value, depth)) = pending.pop() {
        let available = max_tiles.saturating_sub(output.len() + pending.len());
        let node = &tree.nodes[id as usize];
        if remainder
            || !node.is_dir
            || rect.width() < MIN_SIDE
            || rect.height() < MIN_SIDE
            || available < 2
            || depth >= depth_limit
        {
            output.push(Tile {
                id,
                rect,
                remainder,
                weight: value,
                depth,
            });
            continue;
        }
        // Keep the largest children within the remaining budget; group the rest.
        let capacity = available.saturating_sub(1).clamp(1, MAX_CHILDREN);
        let mut kept = BinaryHeap::with_capacity(capacity + 1);
        for child in tree.child_ids(id) {
            let child_value = weight(&tree.nodes[child as usize]);
            if child_value == 0 {
                continue;
            }
            let item = (child_value, child);
            if kept.len() < capacity {
                kept.push(Reverse(item));
            } else if kept
                .peek()
                .is_some_and(|&Reverse(smallest)| item > smallest)
            {
                kept.pop();
                kept.push(Reverse(item));
            }
        }
        let mut items: Vec<(u32, u64)> = kept.into_iter().map(|Reverse((v, id))| (id, v)).collect();
        if items.is_empty() {
            output.push(Tile {
                id,
                rect,
                remainder: false,
                weight: value,
                depth,
            });
            continue;
        }
        items.sort_unstable_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let listed: u64 = items.iter().map(|(_, v)| *v).sum();
        let hidden = value.saturating_sub(listed);
        let grouped = hidden > 0;
        if grouped {
            items.push((id, hidden));
        }
        let mut split = Vec::with_capacity(items.len());
        squarify(&items, rect, &mut split);
        debug_assert_eq!(split.len(), items.len());
        let last = items.len() - 1;
        for index in (0..items.len().min(split.len())).rev() {
            let (child, child_rect) = split[index];
            let (_, child_value) = items[index];
            if !child_rect.is_positive() {
                continue;
            }
            pending.push((
                child,
                child_rect,
                grouped && index == last,
                child_value,
                depth + 1,
            ));
        }
    }
    output
}
/// Lay out weighted items inside `rect` so that every tile stays close to square.
fn squarify(items: &[(u32, u64)], rect: Rect, output: &mut Vec<(u32, Rect)>) {
    let total: u128 = items.iter().map(|(_, value)| u128::from(*value)).sum();
    if total == 0 || !rect.is_positive() {
        for (id, _) in items {
            output.push((*id, rect));
        }
        return;
    }
    let rect_area = f64::from(rect.width()) * f64::from(rect.height());
    let areas: Vec<f64> = items
        .iter()
        .map(|(_, value)| (*value as f64) / (total as f64) * rect_area)
        .collect();
    let mut remaining = rect;
    let mut index = 0;
    while index < items.len() {
        if index == items.len() - 1 {
            // Float drift must not leave a sliver of unpainted area.
            output.push((items[index].0, remaining));
            return;
        }
        let along_width = remaining.width() >= remaining.height();
        let side = if along_width {
            remaining.height()
        } else {
            remaining.width()
        } as f64;
        let mut row = vec![index];
        let mut sum = areas[index];
        let mut next = index + 1;
        while next < items.len() {
            let trial_sum = sum + areas[next];
            let trial_worst = {
                let mut row_areas = row.iter().map(|i| areas[*i]).collect::<Vec<_>>();
                row_areas.push(areas[next]);
                worst_ratio(&row_areas, trial_sum, side)
            };
            if trial_worst
                <= worst_ratio(
                    &row.iter().map(|i| areas[*i]).collect::<Vec<_>>(),
                    sum,
                    side,
                )
            {
                row.push(next);
                sum = trial_sum;
                next += 1;
            } else {
                break;
            }
        }
        let thickness = if side > 0.0 { sum / side } else { 0.0 };
        let mut cursor = if along_width {
            f64::from(remaining.min.y)
        } else {
            f64::from(remaining.min.x)
        };
        for i in &row {
            let length = if thickness > 0.0 {
                areas[*i] / thickness
            } else {
                0.0
            };
            let advanced = cursor + length;
            let end = if *i == row[row.len() - 1] {
                if along_width {
                    f64::from(remaining.max.y)
                } else {
                    f64::from(remaining.max.x)
                }
            } else {
                advanced
            };
            let tile = if along_width {
                Rect::from_min_max(
                    pos2(remaining.min.x, cursor as f32),
                    pos2(remaining.min.x + thickness as f32, end as f32),
                )
            } else {
                Rect::from_min_max(
                    pos2(cursor as f32, remaining.min.y),
                    pos2(end as f32, remaining.min.y + thickness as f32),
                )
            };
            output.push((items[*i].0, tile));
            cursor = advanced;
        }
        remaining = if along_width {
            Rect::from_min_max(
                pos2(remaining.min.x + thickness as f32, remaining.min.y),
                remaining.max,
            )
        } else {
            Rect::from_min_max(
                pos2(remaining.min.x, remaining.min.y + thickness as f32),
                remaining.max,
            )
        };
        index = next;
    }
}
fn worst_ratio(areas: &[f64], sum: f64, side: f64) -> f64 {
    let (Some(maximum), Some(minimum)) = (
        areas.iter().cloned().fold(None, |acc: Option<f64>, value| {
            Some(acc.map_or(value, |acc| acc.max(value)))
        }),
        areas.iter().cloned().fold(None, |acc: Option<f64>, value| {
            Some(acc.map_or(value, |acc| acc.min(value)))
        }),
    ) else {
        return f64::INFINITY;
    };
    if sum <= 0.0 || side <= 0.0 || maximum <= 0.0 || minimum <= 0.0 {
        return f64::INFINITY;
    }
    let side2 = side * side;
    ((side2 * maximum) / (sum * sum)).max((sum * sum) / (side2 * minimum))
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    fn node(
        name: &str,
        parent: u32,
        first_child: u32,
        next_sibling: u32,
        is_dir: bool,
        size: u64,
    ) -> crate::tree::Node {
        crate::tree::Node {
            name: OsString::from(name).into_boxed_os_str(),
            parent,
            first_child,
            next_sibling,
            logical: size,
            allocated: size,
            files: 0,
            directories: 0,
            is_dir,
            error: None,
            complete: true,
        }
    }
    fn logical(node: &crate::tree::Node) -> u64 {
        node.logical
    }
    fn snapshot(nodes: Vec<crate::tree::Node>) -> Snapshot {
        Snapshot {
            engine: "filesystem_walk",
            fallback_reason: None,
            root: "/demo".into(),
            nodes,
        }
    }
    #[test]
    fn equal_children_of_a_square_stay_square() {
        // Four equal directories: slice-and-dice would produce 1:4 slivers.
        let nodes = vec![
            node("", u32::MAX, 1, u32::MAX, true, 4),
            node("a", 0, u32::MAX, 2, true, 1),
            node("b", 0, u32::MAX, 3, true, 1),
            node("c", 0, u32::MAX, 4, true, 1),
            node("d", 0, u32::MAX, u32::MAX, true, 1),
        ];
        let tree = snapshot(nodes);
        let area = Rect::from_min_max(pos2(0.0, 0.0), pos2(200.0, 200.0));
        let tiles = layout(&tree, 0, area, 8, 8, &logical);
        let ratios: Vec<f32> = tiles
            .iter()
            .filter(|t| !t.remainder)
            .map(|t| (t.rect.width() / t.rect.height()).max(t.rect.height() / t.rect.width()))
            .collect();
        assert_eq!(ratios.len(), 4);
        assert!(
            ratios.iter().all(|r| *r < 1.01),
            "squarified layout expected near-square tiles, got {ratios:?}"
        );
    }
    #[test]
    fn many_entries_are_grouped_without_exceeding_tile_budget() {
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
        let tree = snapshot(nodes);
        let area = Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 100.0));
        let tiles = layout(&tree, 0, area, 3, 8, &logical);
        assert_eq!(tiles.len(), 3);
        assert_eq!(tiles.iter().filter(|t| t.remainder).count(), 1);
        let area_sum: f32 = tiles.iter().map(|t| t.rect.width() * t.rect.height()).sum();
        assert!((area_sum - 10000.0).abs() < 1.0, "painted area {area_sum}");
    }
    #[test]
    fn nested_layout_keeps_tiles_inside_and_disjoint() {
        let mut nodes = vec![node("", u32::MAX, 1, u32::MAX, true, 6)];
        // Two directories, each with two files.
        nodes.push(node("dir0", 0, 3, 2, true, 4));
        nodes.push(node("dir1", 0, 5, u32::MAX, true, 2));
        nodes.extend([
            node("f0", 1, u32::MAX, 4, false, 2),
            node("f1", 1, u32::MAX, u32::MAX, false, 2),
            node("f2", 2, u32::MAX, u32::MAX, false, 2),
        ]);
        nodes[2].first_child = 5;
        let tree = snapshot(nodes);
        let area = Rect::from_min_max(pos2(0.0, 0.0), pos2(300.0, 200.0));
        let tiles = layout(&tree, 0, area, 64, 8, &logical);
        assert_eq!(
            tiles.iter().map(|t| t.id).collect::<Vec<_>>(),
            vec![3, 4, 5],
            "a directory is replaced by the entries inside it"
        );
        assert!(
            tiles.iter().all(|t| t.depth == 2),
            "the files sit two levels below the laid-out root"
        );
        for tile in &tiles {
            assert!(
                area.contains_rect(tile.rect),
                "tile {:?} escapes",
                tile.rect
            );
            assert!(tile.rect.is_positive());
        }
        for (i, a) in tiles.iter().enumerate() {
            for b in tiles.iter().skip(i + 1) {
                let overlap = a.rect.intersect(b.rect);
                assert!(
                    !overlap.is_positive() || overlap.area() < 1.0,
                    "{:?} overlaps {:?}",
                    a.rect,
                    b.rect
                );
            }
        }
        let painted: f32 = tiles.iter().map(|t| t.rect.area()).sum();
        assert!((painted - 60000.0).abs() < 2.0, "painted area {painted}");
    }
    #[test]
    fn depth_limit_stops_at_the_requested_level() {
        let mut nodes = vec![node("", u32::MAX, 1, u32::MAX, true, 6)];
        nodes.push(node("dir0", 0, 3, 2, true, 4));
        nodes.push(node("dir1", 0, u32::MAX, u32::MAX, true, 2));
        nodes.push(node("f0", 1, u32::MAX, 4, false, 2));
        nodes.push(node("f1", 1, u32::MAX, u32::MAX, false, 2));
        nodes.push(node("f2", 2, u32::MAX, u32::MAX, false, 2));
        let tree = snapshot(nodes);
        let area = Rect::from_min_max(pos2(0.0, 0.0), pos2(300.0, 200.0));
        let tiles = layout(&tree, 0, area, 64, 1, &logical);
        assert_eq!(
            tiles.iter().map(|t| t.id).collect::<Vec<_>>(),
            vec![1, 2],
            "only the two directories are drawn"
        );
        assert!(tiles.iter().all(|t| t.depth == 1));
    }
    #[test]
    fn squarify_partitions_area_exactly() {
        let area = Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 60.0));
        let mut parts = Vec::new();
        squarify(&[(1, 6), (2, 3), (3, 1)], area, &mut parts);
        assert_eq!(parts.len(), 3);
        let sum: f32 = parts.iter().map(|(_, r)| r.area()).sum();
        assert!((sum - 6000.0).abs() < 0.5, "summed area {sum}");
        for (_, r) in &parts {
            assert!(area.contains_rect(*r));
        }
    }
    #[test]
    fn squarify_avoids_degenerate_aspect_ratios() {
        let weights: Vec<(u32, u64)> = (0..12).map(|i| (i, 12 - i as u64)).collect();
        let area = Rect::from_min_max(pos2(0.0, 0.0), pos2(400.0, 300.0));
        let mut parts = Vec::new();
        squarify(&weights, area, &mut parts);
        let worst = parts
            .iter()
            .map(|(_, r)| (r.width() / r.height()).max(r.height() / r.width()))
            .fold(0.0f32, f32::max);
        assert!(worst < 3.0, "worst aspect ratio {worst}");
    }
}
