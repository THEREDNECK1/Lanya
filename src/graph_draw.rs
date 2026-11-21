use eframe::egui::{self, Color32, Pos2, Stroke, Vec2};
use std::collections::HashMap;

use crate::CommitBranchKind;

pub const LANE_0_X: f32 = 30.0_f32;
pub const LANE_STEP_X: f32 = 45.0_f32;
pub const ROW_HEIGHT: f32 = 36.0_f32;
pub const OUTER_RING_RADIUS: f32 = 5.0_f32;
pub const INNER_DOT_RADIUS: f32 = 2.0_f32;

pub const COLOR_MAIN: Color32 = Color32::from_rgb(56, 189, 248);
pub const COLOR_PASS: Color32 = Color32::from_rgb(45, 212, 191);
pub const COLOR_EXPERIMENT: Color32 = Color32::from_rgb(245, 158, 11);
pub const COLOR_SELECTED: Color32 = Color32::from_rgb(14, 165, 233);

#[derive(Clone, Debug)]
pub struct GraphCommitInput {
    pub full_id: String,
    pub parent_ids: Vec<String>,
    pub lane: usize,
    pub branch_kind: CommitBranchKind,
    pub is_merge: bool,
}

#[derive(Clone, Debug)]
pub struct DagNodeLayout {
    pub x: f32,
    pub y: f32,
    pub color: Color32,
    pub is_merge: bool,
}

#[derive(Clone, Debug)]
pub struct DagEdge {
    pub p0: Pos2,
    pub c1: Pos2,
    pub c2: Pos2,
    pub p3: Pos2,
    pub color: Color32,
    pub is_curve: bool,
}

#[derive(Clone, Debug)]
pub struct DagLayout {
    pub nodes: Vec<DagNodeLayout>,
    pub edges: Vec<DagEdge>,
    pub gutter_width: f32,
    pub total_height: f32,
}

pub fn branch_color(kind: CommitBranchKind) -> Color32 {
    match kind {
        CommitBranchKind::Main => COLOR_MAIN,
        CommitBranchKind::Pass => COLOR_PASS,
        CommitBranchKind::Experiment => COLOR_EXPERIMENT,
    }
}

pub fn lane_to_x(lane: usize) -> f32 {
    LANE_0_X + (lane as f32) * LANE_STEP_X
}

pub fn row_to_y(row: usize) -> f32 {
    (row as f32) * ROW_HEIGHT + ROW_HEIGHT * 0.5_f32
}

pub fn compute_dag_layout(commits: &[GraphCommitInput]) -> DagLayout {
    let mut row_by_id: HashMap<&str, usize> = HashMap::with_capacity(commits.len());
    let mut max_lane: usize = 0;

    for (idx, c) in commits.iter().enumerate() {
        row_by_id.insert(c.full_id.as_str(), idx);
        if c.lane > max_lane {
            max_lane = c.lane;
        }
    }

    let nodes: Vec<DagNodeLayout> = commits
        .iter()
        .enumerate()
        .map(|(row, c)| DagNodeLayout {
            x: lane_to_x(c.lane),
            y: row_to_y(row),
            color: if c.lane >= 2 {
                COLOR_EXPERIMENT
            } else {
                branch_color(c.branch_kind)
            },
            is_merge: c.is_merge,
        })
        .collect();

    let mut edges = Vec::new();
    for (row, c) in commits.iter().enumerate() {
        let p0 = Pos2::new(lane_to_x(c.lane), row_to_y(row));
        for parent_id in &c.parent_ids {
            if let Some(&p_row) = row_by_id.get(parent_id.as_str()) {
                let p_commit = &commits[p_row];
                let p3 = Pos2::new(lane_to_x(p_commit.lane), row_to_y(p_row));
                let is_curve = c.lane != p_commit.lane;
                let color = if c.lane >= 2 {
                    COLOR_EXPERIMENT
                } else if c.lane != 0 {
                    branch_color(c.branch_kind)
                } else if p_commit.lane >= 2 {
                    COLOR_EXPERIMENT
                } else if p_commit.lane != 0 {
                    branch_color(p_commit.branch_kind)
                } else {
                    COLOR_MAIN
                };
                let c1 = Pos2::new(p0.x, p0.y + 15.0_f32);
                let c2 = Pos2::new(p3.x, p3.y - 15.0_f32);
                edges.push(DagEdge {
                    p0,
                    c1,
                    c2,
                    p3,
                    color,
                    is_curve,
                });
            }
        }
    }

    let gutter_width = (lane_to_x(max_lane) + 24.0_f32).max(48.0_f32);
    let total_height = (commits.len() as f32 * ROW_HEIGHT).max(ROW_HEIGHT);

    DagLayout {
        nodes,
        edges,
        gutter_width,
        total_height,
    }
}

pub fn paint_dag(
    ui: &mut egui::Ui,
    layout: &DagLayout,
    commits: &[GraphCommitInput],
    selected_oid: Option<&str>,
) -> Option<String> {
    let (response, painter) = ui.allocate_painter(
        Vec2::new(layout.gutter_width, layout.total_height),
        egui::Sense::click(),
    );
    let origin = response.rect.min.to_vec2();

    // 1. Draw all edges first so node rings render cleanly on top
    for edge in &layout.edges {
        let p0 = edge.p0 + origin;
        let p3 = edge.p3 + origin;
        if !edge.is_curve {
            painter.line_segment([p0, p3], Stroke::new(1.75_f32, edge.color));
        } else {
            let c1 = edge.c1 + origin;
            let c2 = edge.c2 + origin;
            painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
                [p0, c1, c2, p3],
                false,
                Color32::TRANSPARENT,
                Stroke::new(2.5_f32, edge.color),
            ));
        }
    }

    // 2. Draw node rings (5px outer accent ring, 2px white center dot)
    let mut clicked_oid = None;
    let click_pos = if response.clicked() {
        response.interact_pointer_pos()
    } else {
        None
    };

    for (idx, node) in layout.nodes.iter().enumerate() {
        let center = Pos2::new(node.x, node.y) + origin;
        let commit_oid = commits.get(idx).map(|c| c.full_id.as_str());
        let is_selected = commit_oid.is_some() && commit_oid == selected_oid;

        if is_selected {
            painter.circle_stroke(
                center,
                OUTER_RING_RADIUS + 3.0_f32,
                Stroke::new(1.5_f32, COLOR_SELECTED),
            );
        } else if node.is_merge {
            painter.circle_stroke(
                center,
                OUTER_RING_RADIUS + 2.0_f32,
                Stroke::new(1.0_f32, node.color.linear_multiply(0.75_f32)),
            );
        }

        painter.circle_filled(center, OUTER_RING_RADIUS, node.color);
        painter.circle_filled(center, INNER_DOT_RADIUS, Color32::WHITE);

        if let Some(pos) = click_pos {
            if (pos - center).length() <= 14.0_f32 {
                clicked_oid = commit_oid.map(str::to_string);
            }
        }
    }

    clicked_oid
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dag_layout_computes_exact_lanes_rows_and_bezier_edges() {
        let inputs = vec![
            GraphCommitInput {
                full_id: "c3".into(),
                parent_ids: vec!["c2".into(), "b1".into()],
                lane: 0,
                branch_kind: CommitBranchKind::Main,
                is_merge: true,
            },
            GraphCommitInput {
                full_id: "b1".into(),
                parent_ids: vec!["c1".into()],
                lane: 1,
                branch_kind: CommitBranchKind::Pass,
                is_merge: false,
            },
            GraphCommitInput {
                full_id: "c2".into(),
                parent_ids: vec!["c1".into()],
                lane: 0,
                branch_kind: CommitBranchKind::Main,
                is_merge: false,
            },
            GraphCommitInput {
                full_id: "c1".into(),
                parent_ids: vec![],
                lane: 0,
                branch_kind: CommitBranchKind::Main,
                is_merge: false,
            },
        ];

        let layout = compute_dag_layout(&inputs);
        assert_eq!(layout.nodes.len(), 4);
        assert_eq!(layout.nodes[0].x, 30.0_f32);
        assert_eq!(layout.nodes[0].y, 18.0_f32);
        assert_eq!(layout.nodes[1].x, 75.0_f32);
        assert_eq!(layout.nodes[1].y, 54.0_f32);
        assert_eq!(layout.nodes[2].y, 90.0_f32);
        assert_eq!(layout.nodes[3].y, 126.0_f32);

        let curve_count = layout.edges.iter().filter(|e| e.is_curve).count();
        let straight_count = layout.edges.iter().filter(|e| !e.is_curve).count();
        assert_eq!(curve_count, 2); // merge c3->b1 and fork b1->c1
        assert_eq!(straight_count, 2); // c3->c2 and c2->c1
    }
}
