use crate::graph_draw::{self, COLOR_EXPERIMENT};
use crate::{AppView, CommitBranchKind, CommitNodeInfo, ResolveSidecarApp, StorageMountState};
use eframe::egui::{
    self, Align, Color32, Frame, Label, Layout, Margin, RichText, Rounding, ScrollArea, Stroke,
    Vec2,
};
use std::collections::{HashMap, HashSet};

const BURST_WINDOW_SECS: i64 = 90;

#[derive(Clone)]
pub struct CommitGroup {
    pub id: String,                    // Hash of the newest commit in group
    pub representative_hash: String,
    pub branch: String,
    pub is_expanded: bool,
    pub start_time: i64,
    pub end_time: i64,
    pub primary_category: String,      // "[Cut & Mix]", "[Audio]", or "[Edit]"
    pub commits: Vec<CommitNodeInfo>,  // The underlying raw commits
}

fn commit_branch_label(c: &CommitNodeInfo) -> String {
    if let Some(ref b) = c.branch_name {
        return b.clone();
    }
    match c.branch_kind {
        CommitBranchKind::Main => "main".to_string(),
        CommitBranchKind::Pass => "pass".to_string(),
        CommitBranchKind::Experiment => "experiment".to_string(),
    }
}

fn primary_category_for_commits(commits: &[CommitNodeInfo]) -> String {
    let mut has_audio = false;
    let mut has_video = false;
    for c in commits {
        let msg = c.message.trim();
        if msg.starts_with("[Cut & Mix]") {
            has_audio = true;
            has_video = true;
        } else if msg.starts_with("[Audio]") {
            has_audio = true;
        } else if msg.starts_with("[Edit]") {
            has_video = true;
        } else {
            let lower = msg.to_ascii_lowercase();
            if lower.contains("audio") || lower.contains(".wav") || lower.contains(".mp3") {
                has_audio = true;
            } else {
                has_video = true;
            }
        }
    }
    if has_audio && has_video {
        "[Cut & Mix]".to_string()
    } else if has_audio {
        "[Audio]".to_string()
    } else {
        "[Edit]".to_string()
    }
}

fn burst_anchor_id(commits: &[CommitNodeInfo]) -> String {
    commits
        .last()
        .map(|c| c.full_id.clone())
        .unwrap_or_default()
}

pub fn build_commit_groups(
    commits: &[CommitNodeInfo],
    expanded_group_ids: &HashSet<String>,
) -> Vec<CommitGroup> {
    if commits.is_empty() {
        return Vec::new();
    }

    let mut raw_groups: Vec<Vec<CommitNodeInfo>> = Vec::new();
    for commit in commits {
        let can_merge = if let Some(cur) = raw_groups.last() {
            let prev = &cur[cur.len() - 1];
            let same_lane = prev.lane == commit.lane;
            let same_branch = commit_branch_label(prev) == commit_branch_label(commit);
            let neither_merge = !prev.is_merge && !commit.is_merge;
            let direct_chain = prev
                .parent_ids
                .first()
                .is_some_and(|pid| pid == &commit.full_id);
            let within_burst = (prev.timestamp_secs - commit.timestamp_secs).abs() <= BURST_WINDOW_SECS;
            let no_tag_hidden = commit.tag.is_none();
            same_lane && same_branch && neither_merge && direct_chain && within_burst && no_tag_hidden
        } else {
            false
        };

        if can_merge {
            if let Some(cur) = raw_groups.last_mut() {
                cur.push(commit.clone());
            }
        } else {
            raw_groups.push(vec![commit.clone()]);
        }
    }

    raw_groups
        .into_iter()
        .map(|bucket| {
            let newest = &bucket[0];
            let id = newest.full_id.clone();
            let representative_hash = newest.short_id.clone();
            let branch = commit_branch_label(newest);
            let anchor = burst_anchor_id(&bucket);
            let is_expanded = bucket.len() > 1
                && (expanded_group_ids.contains(&anchor) || expanded_group_ids.contains(&id));
            let start_time = bucket
                .iter()
                .map(|c| c.timestamp_secs)
                .min()
                .unwrap_or(newest.timestamp_secs);
            let end_time = bucket
                .iter()
                .map(|c| c.timestamp_secs)
                .max()
                .unwrap_or(newest.timestamp_secs);
            let primary_category = primary_category_for_commits(&bucket);

            CommitGroup {
                id,
                representative_hash,
                branch,
                is_expanded,
                start_time,
                end_time,
                primary_category,
                commits: bucket,
            }
        })
        .collect()
}

pub fn format_group_header(group: &CommitGroup) -> String {
    let mut tracks: Vec<String> = Vec::new();
    for c in &group.commits {
        for t in &c.tracks {
            if !tracks.contains(t) {
                tracks.push(t.clone());
            }
        }
    }
    tracks.sort();

    let tracks_str = if tracks.is_empty() {
        match group.primary_category.as_str() {
            "[Cut & Mix]" => "A1, V1".to_string(),
            "[Audio]" => "A1".to_string(),
            _ => "V1".to_string(),
        }
    } else {
        tracks.join(", ")
    };

    match group.primary_category.as_str() {
        "[Cut & Mix]" => format!(
            "[Session: Edit & Mix] {} changes ({tracks_str})",
            group.commits.len()
        ),
        "[Audio]" => format!(
            "[Audio Pass] {} clips modified ({tracks_str})",
            group.commits.len()
        ),
        _ => format!(
            "[Video Cut] {} clips modified ({tracks_str})",
            group.commits.len()
        ),
    }
}

fn clean_leaf_commit_summary(raw: &str) -> String {
    let trimmed = raw
        .trim()
        .trim_start_matches("[Edit]")
        .trim_start_matches("[Audio]")
        .trim_start_matches("[Cut & Mix]")
        .trim_start_matches("[Sync]")
        .trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("Timeline snapshot") {
        "Adjusted timeline cut".to_string()
    } else {
        trimmed.to_string()
    }
}

const BG_BASE: Color32 = Color32::from_rgb(13, 13, 15);
const BG_SURFACE: Color32 = Color32::from_rgb(22, 22, 26);
const BG_ELEVATED: Color32 = Color32::from_rgb(31, 31, 37);
const BORDER_SUBTLE: Color32 = Color32::from_rgb(48, 48, 58);

const TEXT_PRIMARY: Color32 = Color32::from_rgb(244, 244, 246);
const TEXT_SECONDARY: Color32 = Color32::from_rgb(161, 161, 175);
const TEXT_MUTED: Color32 = Color32::from_rgb(113, 113, 128);

const ACCENT_CYAN: Color32 = Color32::from_rgb(96, 165, 250);
const ACCENT_CYAN_DARK: Color32 = Color32::from_rgb(37, 99, 235);
const ACCENT_TEAL: Color32 = Color32::from_rgb(52, 211, 153);
const ACCENT_TEAL_BTN: Color32 = Color32::from_rgb(16, 145, 108);
const ACCENT_AMBER: Color32 = Color32::from_rgb(245, 179, 66);
const ACCENT_RED: Color32 = Color32::from_rgb(248, 113, 113);

fn card_shadow() -> egui::epaint::Shadow {
    egui::epaint::Shadow {
        offset: Vec2::new(0.0, 4.0),
        blur: 14.0,
        spread: 0.0,
        color: Color32::from_rgba_unmultiplied(0, 0, 0, 110),
    }
}

fn google_g_texture(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let id = egui::Id::new("google_g_icon_tex");
    if let Some(tex) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return Some(tex);
    }
    let icon_data = eframe::icon_data::from_png_bytes(include_bytes!("../assets/google_g.png")).ok()?;
    let color_img = egui::ColorImage::from_rgba_unmultiplied(
        [icon_data.width as usize, icon_data.height as usize],
        &icon_data.rgba,
    );
    let tex = ctx.load_texture("google_g_icon", color_img, egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, tex.clone()));
    Some(tex)
}

fn render_google_sign_in_button(ui: &mut egui::Ui, enabled: bool) -> egui::Response {
    let btn_size = Vec2::new(192.0_f32, 36.0_f32);
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(btn_size, sense);
    let bg = if !enabled {
        Color32::from_rgb(24, 24, 26)
    } else if resp.hovered() {
        Color32::from_rgb(31, 31, 31)
    } else {
        Color32::from_rgb(19, 19, 20)
    };
    let border_col = Color32::from_rgb(116, 119, 117);
    let text_col = if enabled {
        Color32::from_rgb(227, 227, 227)
    } else {
        TEXT_MUTED
    };

    let painter = ui.painter();
    painter.rect_filled(rect, Rounding::same(18.0_f32), bg);
    painter.rect_stroke(rect, Rounding::same(18.0_f32), Stroke::new(1.0_f32, border_col));

    let icon_rect = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 21.0_f32, rect.center().y),
        Vec2::splat(18.0_f32),
    );
    if let Some(tex) = google_g_texture(ui.ctx()) {
        painter.image(
            tex.id(),
            icon_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    }

    painter.text(
        egui::pos2(icon_rect.right() + 8.0_f32, rect.center().y),
        egui::Align2::LEFT_CENTER,
        "Sign in with Google",
        egui::FontId::proportional(12.0_f32),
        text_col,
    );

    resp
}

fn surface_card() -> Frame {
    Frame::none()
        .fill(BG_SURFACE)
        .rounding(Rounding::same(10.0_f32))
        .stroke(Stroke::new(1.0_f32, BORDER_SUBTLE))
        .shadow(card_shadow())
        .inner_margin(Margin::same(16.0_f32))
}

pub(crate) fn render_app(app: &mut ResolveSidecarApp, ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.visuals.dark_mode = true;
    style.visuals.panel_fill = BG_BASE;
    style.visuals.window_fill = BG_SURFACE;
    style.visuals.extreme_bg_color = Color32::from_rgb(16, 16, 20);
    style.visuals.faint_bg_color = Color32::from_rgb(26, 26, 31);
    style.visuals.widgets.noninteractive.bg_fill = BG_SURFACE;
    style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER_SUBTLE);
    style.visuals.widgets.noninteractive.rounding = Rounding::same(8.0_f32);
    style.visuals.widgets.inactive.bg_fill = BG_ELEVATED;
    style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, BORDER_SUBTLE);
    style.visuals.widgets.inactive.rounding = Rounding::same(6.0_f32);
    style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(42, 42, 50);
    style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(84, 84, 102));
    style.visuals.widgets.hovered.rounding = Rounding::same(6.0_f32);
    style.visuals.widgets.active.bg_fill = Color32::from_rgb(50, 50, 60);
    style.visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, ACCENT_CYAN);
    style.visuals.widgets.active.rounding = Rounding::same(6.0_f32);
    style.visuals.selection.bg_fill = Color32::from_rgba_unmultiplied(59, 130, 246, 55);
    style.visuals.selection.stroke = Stroke::new(1.0_f32, ACCENT_CYAN);
    style.visuals.window_shadow = card_shadow();
    style.visuals.popup_shadow = card_shadow();
    style.visuals.window_rounding = Rounding::same(10.0_f32);
    style.spacing.button_padding = Vec2::new(11.0_f32, 5.5_f32);
    style.spacing.item_spacing = Vec2::new(8.0_f32, 7.0_f32);
    ctx.set_style(style);

    let repo_label = app
        .repo_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("Resolve_Cloud_Storage")
        .to_string();

    // Top Bar
    egui::TopBottomPanel::top("cinema_topbar")
        .exact_height(46.0_f32)
        .frame(
            Frame::none()
                .fill(BG_SURFACE)
                .stroke(Stroke::new(1.0_f32, BORDER_SUBTLE))
                .inner_margin(Margin::symmetric(14.0_f32, 8.0_f32)),
        )
        .show(ctx, |ui| {
            let avail_w = ui.available_width();
            ui.set_max_width(avail_w);
            ui.horizontal_centered(|ui| {
                ui.label(
                    RichText::new("LANYA")
                        .strong()
                        .size(13.0_f32)
                        .color(TEXT_PRIMARY),
                );
                ui.add_space(8.0_f32);

                let branch_color = if app.current_branch == "main" {
                    ACCENT_CYAN
                } else if app.current_branch.starts_with("experiment/") {
                    ACCENT_AMBER
                } else {
                    ACCENT_TEAL
                };
                    Frame::none()
                    .fill(BG_ELEVATED)
                    .rounding(Rounding::same(4.0_f32))
                    .stroke(Stroke::new(1.0_f32, branch_color.linear_multiply(0.55_f32)))
                    .inner_margin(Margin::symmetric(7.0_f32, 3.0_f32))
                    .show(ui, |ui| {
                        let pill_avail = ui.available_width();
                        ui.set_max_width(pill_avail);
                        ui.label(
                            RichText::new(format!("{repo_label} : {}", app.current_branch))
                                .monospace()
                                .size(10.5_f32)
                                .color(branch_color),
                        );
                    });

                ui.add_space(6.0_f32);
                if app.z_drive_connected {
                    ui.label(
                        RichText::new("\u{25cf} Z: Connected")
                            .monospace()
                            .size(10.0_f32)
                            .color(Color32::from_rgb(74, 222, 128)),
                    );
                } else {
                    ui.label(
                        RichText::new("\u{25cf} Z: Disconnected")
                            .monospace()
                            .size(10.0_f32)
                            .color(ACCENT_AMBER),
                    );
                    let mount_btn = egui::Button::new(
                        RichText::new("Mount Z:")
                            .strong()
                            .size(9.5_f32)
                            .color(Color32::WHITE),
                    )
                    .fill(ACCENT_CYAN_DARK)
                    .rounding(Rounding::same(3.0_f32));
                    if ui.add(mount_btn).clicked() {
                        app.mount_z_drive_now();
                    }
                }

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let sync_btn = egui::Button::new(
                        RichText::new("Sync")
                            .strong()
                            .size(11.0_f32)
                            .color(Color32::WHITE),
                    )
                    .fill(ACCENT_CYAN_DARK)
                    .rounding(Rounding::same(4.0_f32));
                    if ui.add(sync_btn).clicked() {
                        app.sync_repo_now();
                    }

                    let open_tickets = app
                        .tickets
                        .iter()
                        .filter(|t| t.status == crate::tickets::TicketStatus::Open)
                        .count();
                    let ticket_label = if open_tickets > 0 {
                        format!("\u{1f4ac} Tickets ({open_tickets})")
                    } else {
                        "\u{1f4ac} Tickets".to_string()
                    };
                    let tickets_btn = egui::Button::new(
                        RichText::new(ticket_label)
                            .strong()
                            .size(11.0_f32)
                            .color(if app.show_tickets_drawer {
                                Color32::WHITE
                            } else {
                                TEXT_PRIMARY
                            }),
                    )
                    .fill(if app.show_tickets_drawer {
                        ACCENT_CYAN_DARK
                    } else {
                        BG_ELEVATED
                    })
                    .stroke(Stroke::new(
                        1.0_f32,
                        if open_tickets > 0 {
                            ACCENT_AMBER.linear_multiply(0.75_f32)
                        } else {
                            BORDER_SUBTLE
                        },
                    ))
                    .rounding(Rounding::same(4.0_f32));
                    if ui.add(tickets_btn).clicked() {
                        app.toggle_tickets_drawer();
                    }

                    let fork_btn = egui::Button::new(
                        RichText::new("+ Fork Sandbox Cut")
                            .strong()
                            .size(11.0_f32)
                            .color(Color32::from_rgb(15, 23, 42)),
                    )
                    .fill(ACCENT_AMBER)
                    .rounding(Rounding::same(4.0_f32));
                    if ui.add(fork_btn).clicked() {
                        app.show_fork_sandbox_modal = true;
                    }
                });
            });
        });

fn draw_nav_icon(painter: &egui::Painter, center: egui::Pos2, view: AppView, color: Color32) {
    let stroke = Stroke::new(1.5_f32, color);
    match view {
        AppView::HomeProjects => {
            // Home glyph: pitched roof + house body
            let roof = vec![
                egui::pos2(center.x - 6.0, center.y - 0.5),
                egui::pos2(center.x, center.y - 5.5),
                egui::pos2(center.x + 6.0, center.y - 0.5),
            ];
            painter.add(egui::Shape::line(roof, stroke));
            let body = egui::Rect::from_min_max(
                egui::pos2(center.x - 4.0, center.y - 1.0),
                egui::pos2(center.x + 4.0, center.y + 5.0),
            );
            painter.rect_stroke(body, Rounding::same(1.0_f32), stroke);
        }
        AppView::MediaPipeline => {
            // Storage/Pipeline nodes glyph: two stacked server/node bars connected
            let top_box = egui::Rect::from_min_max(
                egui::pos2(center.x - 5.5, center.y - 5.0),
                egui::pos2(center.x + 5.5, center.y - 1.0),
            );
            let bot_box = egui::Rect::from_min_max(
                egui::pos2(center.x - 5.5, center.y + 1.0),
                egui::pos2(center.x + 5.5, center.y + 5.0),
            );
            painter.rect_stroke(top_box, Rounding::same(1.0_f32), stroke);
            painter.rect_stroke(bot_box, Rounding::same(1.0_f32), stroke);
            painter.circle_filled(egui::pos2(center.x - 3.0, center.y - 3.0), 1.2_f32, color);
            painter.circle_filled(egui::pos2(center.x - 3.0, center.y + 3.0), 1.2_f32, color);
        }
        AppView::Timeline => {
            // Branch/Fork glyph: vertical stem + curved branch + 3 nodes
            let p_top = egui::pos2(center.x - 3.5, center.y - 4.5);
            let p_bot = egui::pos2(center.x - 3.5, center.y + 4.5);
            let p_fork = egui::pos2(center.x - 3.5, center.y + 1.0);
            let p_right = egui::pos2(center.x + 4.0, center.y - 3.5);
            painter.line_segment([p_top, p_bot], stroke);
            painter.line_segment([p_fork, p_right], stroke);
            painter.circle_filled(p_top, 1.8_f32, color);
            painter.circle_filled(p_bot, 1.8_f32, color);
            painter.circle_filled(p_right, 1.8_f32, color);
        }
    }
}

    // Suite Navigation Strip
    egui::TopBottomPanel::top("cinema_navstrip")
        .exact_height(40.0_f32)
        .frame(
            Frame::none()
                .fill(BG_BASE)
                .stroke(Stroke::new(1.0_f32, BORDER_SUBTLE))
                .inner_margin(Margin::symmetric(14.0_f32, 4.0_f32)),
        )
        .show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0_f32;
                for view in AppView::ALL {
                    let active = app.current_view == view;
                    let text_color = if active { TEXT_PRIMARY } else { TEXT_SECONDARY };
                    let icon_color = if active { ACCENT_CYAN } else { TEXT_SECONDARY };
                    let label = view.label();
                    let est_w = 28.0_f32 + (label.len() as f32 * 6.8_f32) + 16.0_f32;
                    let (rect, resp) = ui.allocate_exact_size(
                        Vec2::new(est_w, 30.0_f32),
                        egui::Sense::click(),
                    );
                    let bg = if active {
                        BG_ELEVATED
                    } else if resp.hovered() {
                        Color32::from_rgb(26, 26, 32)
                    } else {
                        Color32::TRANSPARENT
                    };
                    let painter = ui.painter();
                    painter.rect_filled(rect, Rounding::same(6.0_f32), bg);
                    if active {
                        painter.rect_stroke(
                            rect,
                            Rounding::same(6.0_f32),
                            Stroke::new(1.0_f32, BORDER_SUBTLE),
                        );
                        // Cyan underline indicator
                        painter.line_segment(
                            [
                                egui::pos2(rect.left() + 8.0, rect.bottom() - 1.5),
                                egui::pos2(rect.right() - 8.0, rect.bottom() - 1.5),
                            ],
                            Stroke::new(2.0_f32, ACCENT_CYAN),
                        );
                    }
                    let icon_center = egui::pos2(rect.left() + 16.0, rect.center().y);
                    draw_nav_icon(painter, icon_center, view, icon_color);
                    painter.text(
                        egui::pos2(rect.left() + 28.0, rect.center().y),
                        egui::Align2::LEFT_CENTER,
                        label,
                        egui::FontId::proportional(11.5_f32),
                        text_color,
                    );
                    if resp.clicked() {
                        app.current_view = view;
                    }
                }
            });
        });

    // Bottom Status Bar
    egui::TopBottomPanel::bottom("cinema_statusbar")
        .exact_height(24.0_f32)
        .frame(
            Frame::none()
                .fill(BG_SURFACE)
                .stroke(Stroke::new(1.0_f32, BORDER_SUBTLE))
                .inner_margin(Margin::symmetric(14.0_f32, 3.0_f32)),
        )
        .show(ctx, |ui| {
            let avail_w = ui.available_width();
            ui.set_max_width(avail_w);
            ui.horizontal(|ui| {
                let left_w = (avail_w * 0.50_f32).max(160.0_f32);
                ui.allocate_ui(Vec2::new(left_w, 18.0_f32), |ui| {
                    ui.add(
                        Label::new(
                            RichText::new(&app.status_message)
                                .monospace()
                                .size(10.5_f32)
                                .color(TEXT_PRIMARY),
                        )
                        .truncate(true),
                    );
                });

                let storage_text = match app.storage_state {
                    StorageMountState::MountedZ => "Z:\\ Mounted",
                    StorageMountState::MountingZ => "Mounting Z:\\...",
                    StorageMountState::LocalProject => "Local Repo",
                };
                ui.label(
                    RichText::new(format!("• {storage_text}"))
                        .monospace()
                        .size(10.5_f32)
                        .color(TEXT_SECONDARY),
                );

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if let Ok(ai_guard) = app.ai_model_state.lock() {
                        let (ai_text, ai_color) = match *ai_guard {
                            crate::ai_engine::AiModelState::Downloading => {
                                ("AI Summarizer: Downloading...", ACCENT_AMBER)
                            }
                            crate::ai_engine::AiModelState::Ready(_) => {
                                ("AI Summarizer: Ready", ACCENT_TEAL)
                            }
                            crate::ai_engine::AiModelState::Fallback => {
                                ("AI Summarizer: Rule-Based", TEXT_MUTED)
                            }
                        };
                        ui.label(
                            RichText::new(ai_text)
                                .monospace()
                                .size(10.5_f32)
                                .color(ai_color),
                        );
                    }
                });
            });
        });

    // Fork Sandbox Cut Modal
    if app.show_fork_sandbox_modal {
        let mut open = app.show_fork_sandbox_modal;
        egui::Window::new("Fork Sandbox Cut")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .fixed_size(Vec2::new(360.0_f32, 125.0_f32))
            .show(ctx, |ui| {
                let avail_w = ui.available_width();
                ui.set_max_width(avail_w);
                ui.label(
                    RichText::new("Sandbox Name (creates experiment/<name>)")
                        .size(11.5_f32)
                        .color(TEXT_SECONDARY),
                );
                ui.add_space(4.0_f32);
                ui.add(
                    egui::TextEdit::singleline(&mut app.fork_sandbox_name)
                        .desired_width(330.0_f32)
                        .hint_text("e.g., cold-open"),
                );
                ui.add_space(10.0_f32);
                ui.horizontal(|ui| {
                    let go_btn = egui::Button::new(
                        RichText::new("Fork & Open in Resolve")
                            .strong()
                            .size(11.5_f32)
                            .color(Color32::from_rgb(15, 23, 42)),
                    )
                    .fill(ACCENT_AMBER)
                    .rounding(Rounding::same(4.0_f32));
                    if ui.add(go_btn).clicked() {
                        app.on_click_fork_sandbox_cut();
                        app.show_fork_sandbox_modal = false;
                    }
                    if ui.button("Cancel").clicked() {
                        app.show_fork_sandbox_modal = false;
                    }
                });
            });
        if !open {
            app.show_fork_sandbox_modal = false;
        }
    }

    if app.show_new_project_modal {
        let mut open = true;
        egui::Window::new("New Project Scaffolder")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .fixed_size(Vec2::new(400.0_f32, 185.0_f32))
            .show(ctx, |ui| {
                ui.label(
                    RichText::new("Project Name")
                        .strong()
                        .size(11.0_f32)
                        .color(TEXT_PRIMARY),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut app.new_project_name)
                        .hint_text("e.g. Client_Documentary_Cut")
                        .desired_width(370.0_f32),
                );
                ui.add_space(8.0_f32);
                ui.label(
                    RichText::new("Base Directory")
                        .strong()
                        .size(11.0_f32)
                        .color(TEXT_PRIMARY),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut app.new_project_base_dir)
                        .desired_width(370.0_f32),
                );
                ui.add_space(12.0_f32);
                ui.horizontal(|ui| {
                    let create_btn = egui::Button::new(
                        RichText::new("Create & Open Project")
                            .strong()
                            .size(11.5_f32)
                            .color(Color32::WHITE),
                    )
                    .fill(ACCENT_TEAL_BTN)
                    .rounding(Rounding::same(5.0_f32));
                    if ui.add(create_btn).clicked() {
                        app.create_and_open_project();
                    }
                    if ui.button("Cancel").clicked() {
                        app.show_new_project_modal = false;
                    }
                });
            });
        if !open {
            app.show_new_project_modal = false;
        }
    }

    egui::CentralPanel::default()
        .frame(
            Frame::none()
                .fill(BG_BASE)
                .inner_margin(Margin::same(12.0_f32)),
        )
        .show(ctx, |ui| {
            let avail_w = ui.available_width();
            ui.set_max_width(avail_w);
            match app.current_view {
                AppView::HomeProjects => render_home_projects_view(app, ui),
                AppView::MediaPipeline => render_media_pipeline_view(app, ui),
                AppView::Timeline => {
                    if app.needs_repo_setup {
                        render_first_run_setup_card(app, ui);
                    } else {
                        render_workspace_view(app, ui);
                    }
                }
            }
        });
}

fn render_home_projects_view(app: &mut ResolveSidecarApp, ui: &mut egui::Ui) {
    ScrollArea::vertical()
        .id_source("home_collab_scroll")
        .show(ui, |ui| {
            if app.needs_repo_setup {
                render_first_run_setup_card(app, ui);
                ui.add_space(16.0_f32);
            }

            ui.columns(2, |cols| {
                // Left Column: Active Project Card + Z:\Projects Grid + [+ New Project]
                cols[0].vertical(|ui| {
                    let active_name = app
                        .repo_path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("Resolve_Cloud_Storage")
                        .to_string();
                    Frame::none()
                        .fill(BG_SURFACE)
                        .rounding(Rounding::same(10.0_f32))
                        .stroke(Stroke::new(1.0_f32, ACCENT_CYAN.linear_multiply(0.55_f32)))
                        .shadow(card_shadow())
                        .inner_margin(Margin::same(16.0_f32))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new("ACTIVE PROJECT")
                                    .strong()
                                    .size(10.5_f32)
                                    .color(ACCENT_CYAN),
                            );
                            ui.add_space(4.0_f32);
                            ui.horizontal(|ui| {
                                ui.vertical(|ui| {
                                    ui.label(
                                        RichText::new(&active_name)
                                            .strong()
                                            .size(14.0_f32)
                                            .color(TEXT_PRIMARY),
                                    );
                                    ui.label(
                                        RichText::new(format!(
                                            "{} \u{2022} branch: {}",
                                            app.repo_path.display(),
                                            app.current_branch
                                        ))
                                        .monospace()
                                        .size(10.0_f32)
                                        .color(TEXT_MUTED),
                                    );
                                });
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    let tl_btn = egui::Button::new(
                                        RichText::new("Open Timeline")
                                            .strong()
                                            .size(11.0_f32)
                                            .color(Color32::WHITE),
                                    )
                                    .fill(ACCENT_TEAL_BTN)
                                    .rounding(Rounding::same(4.0_f32));
                                    if ui.add(tl_btn).clicked() {
                                        app.current_view = AppView::Timeline;
                                    }
                                });
                            });
                        });

                    ui.add_space(10.0_f32);

                    surface_card().show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("PROJECT GRID (Z:\\PROJECTS & RECENT)")
                                    .strong()
                                    .size(11.0_f32)
                                    .color(TEXT_SECONDARY),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let new_btn = egui::Button::new(
                                    RichText::new("+ New Project")
                                        .strong()
                                        .size(11.0_f32)
                                        .color(Color32::WHITE),
                                )
                                .fill(ACCENT_CYAN_DARK)
                                .rounding(Rounding::same(4.0_f32));
                                if ui.add(new_btn).clicked() {
                                    if std::path::Path::new(r"Z:\Projects").is_dir() {
                                        app.new_project_base_dir = r"Z:\Projects".to_string();
                                    }
                                    app.show_new_project_modal = true;
                                }
                            });
                        });
                        ui.add_space(8.0_f32);

                        let list = app.scan_z_projects();
                        if list.is_empty() {
                            ui.label(
                                RichText::new("No projects found yet. Click '+ New Project' to create one.")
                                    .size(11.0_f32)
                                    .color(TEXT_MUTED),
                            );
                        } else {
                            for proj in list {
                                let is_active =
                                    proj.path == app.repo_path && !app.needs_repo_setup;
                                Frame::none()
                                    .fill(BG_ELEVATED)
                                    .rounding(Rounding::same(6.0_f32))
                                    .stroke(Stroke::new(1.0_f32, BORDER_SUBTLE))
                                    .inner_margin(Margin::same(10.0_f32))
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.vertical(|ui| {
                                                ui.label(
                                                    RichText::new(&proj.name)
                                                        .strong()
                                                        .size(12.0_f32)
                                                        .color(TEXT_PRIMARY),
                                                );
                                                ui.label(
                                                    RichText::new(format!(
                                                        "{} \u{2022} {}",
                                                        proj.path.display(),
                                                        proj.last_opened
                                                    ))
                                                    .monospace()
                                                    .size(9.5_f32)
                                                    .color(TEXT_MUTED),
                                                );
                                            });
                                            ui.with_layout(
                                                Layout::right_to_left(Align::Center),
                                                |ui| {
                                                    let open_btn = egui::Button::new(
                                                        RichText::new(if is_active {
                                                            "Active"
                                                        } else {
                                                            "Open"
                                                        })
                                                        .strong()
                                                        .size(10.5_f32)
                                                        .color(Color32::WHITE),
                                                    )
                                                    .fill(if is_active {
                                                        ACCENT_TEAL_BTN
                                                    } else {
                                                        ACCENT_CYAN_DARK
                                                    })
                                                    .rounding(Rounding::same(4.0_f32));
                                                    if ui.add(open_btn).clicked() {
                                                        app.switch_project(proj.path.clone());
                                                    }
                                                },
                                            );
                                        });
                                    });
                                ui.add_space(6.0_f32);
                            }
                        }
                    });
                });

                // Right Column: GitHub Collaboration & Identity
                surface_card()
                    .show(&mut cols[1], |ui| {
                        ui.label(
                            RichText::new("GITHUB COLLABORATION & IDENTITY")
                                .strong()
                                .size(11.5_f32)
                                .color(TEXT_PRIMARY),
                        );
                        ui.add_space(10.0_f32);

                        ui.label(RichText::new("Editor Name").strong().size(11.0_f32).color(TEXT_SECONDARY));
                        ui.add(
                            egui::TextEdit::singleline(&mut app.github_auth.editor_name)
                                .desired_width(ui.available_width()),
                        );
                        ui.add_space(8.0_f32);

                        ui.label(RichText::new("Editor Email").strong().size(11.0_f32).color(TEXT_SECONDARY));
                        ui.add(
                            egui::TextEdit::singleline(&mut app.github_auth.editor_email)
                                .desired_width(ui.available_width()),
                        );
                        ui.add_space(8.0_f32);

                        ui.label(RichText::new("Remote Origin URL").strong().size(11.0_f32).color(TEXT_SECONDARY));
                        ui.add(
                            egui::TextEdit::singleline(&mut app.github_auth.remote_url)
                                .desired_width(ui.available_width()),
                        );
                        ui.add_space(8.0_f32);

                        ui.label(
                            RichText::new("GitHub Personal Access Token (PAT, optional)")
                                .strong()
                                .size(11.0_f32)
                                .color(TEXT_SECONDARY),
                        );
                        ui.add(
                            egui::TextEdit::singleline(&mut app.github_auth.pat_token)
                                .password(true)
                                .desired_width(ui.available_width()),
                        );
                        ui.add_space(8.0_f32);

                        let adv_label = if app.github_auth.show_advanced {
                            "v Advanced Settings (OAuth App)"
                        } else {
                            "> Advanced Settings (OAuth App)"
                        };
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new(adv_label)
                                        .size(10.0_f32)
                                        .color(TEXT_MUTED),
                                )
                                .frame(false),
                            )
                            .clicked()
                        {
                            app.github_auth.show_advanced = !app.github_auth.show_advanced;
                        }

                        if app.github_auth.show_advanced {
                            ui.add_space(4.0_f32);
                            ui.horizontal(|ui| {
                                ui.vertical(|ui| {
                                    ui.label(
                                        RichText::new("GitHub OAuth Client ID")
                                            .strong()
                                            .size(10.5_f32)
                                            .color(TEXT_SECONDARY),
                                    );
                                    ui.add(
                                        egui::TextEdit::singleline(&mut app.github_auth.oauth_client_id)
                                            .hint_text("Ov23li...")
                                            .desired_width(210.0_f32),
                                    );
                                });
                                ui.vertical(|ui| {
                                    ui.label(
                                        RichText::new("OAuth Client Secret")
                                            .strong()
                                            .size(10.5_f32)
                                            .color(TEXT_SECONDARY),
                                    );
                                    ui.add(
                                        egui::TextEdit::singleline(&mut app.github_auth.oauth_client_secret)
                                            .password(true)
                                            .desired_width(210.0_f32),
                                    );
                                });
                            });
                            ui.label(
                                RichText::new("OAuth Redirect URI: http://127.0.0.1:8088/callback")
                                    .monospace()
                                    .size(9.5_f32)
                                    .color(TEXT_MUTED),
                            );
                        }
                        ui.add_space(12.0_f32);

                        ui.horizontal_wrapped(|ui| {
                            if render_google_sign_in_button(ui, !app.github_oauth_in_progress)
                                .clicked()
                            {
                                app.start_github_oauth();
                            }

                            let save_btn = egui::Button::new(
                                RichText::new("Save Settings")
                                    .strong()
                                    .size(11.0_f32)
                                    .color(Color32::WHITE),
                            )
                            .fill(ACCENT_TEAL_BTN)
                            .rounding(Rounding::same(4.0_f32));
                            if ui.add(save_btn).clicked() {
                                app.save_github_collab_settings();
                            }

                            let verify_btn = egui::Button::new(
                                RichText::new("Verify Auth")
                                    .strong()
                                    .size(11.0_f32)
                                    .color(Color32::WHITE),
                            )
                            .fill(ACCENT_CYAN_DARK)
                            .rounding(Rounding::same(4.0_f32));
                            if ui.add(verify_btn).clicked() {
                                app.verify_github_auth();
                            }
                        });

                        ui.add_space(4.0_f32);
                        ui.label(
                            RichText::new("Opens GitHub in your browser (supports GitHub or Continue with Google).")
                                .size(10.0_f32)
                                .color(TEXT_MUTED),
                        );

                        if app.github_oauth_in_progress {
                            ui.add_space(6.0_f32);
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label(
                                    RichText::new(
                                        "Listening on http://127.0.0.1:8088/callback...",
                                    )
                                    .size(10.5_f32)
                                    .color(ACCENT_AMBER),
                                );
                            });
                        } else if !app.github_auth.auth_status_note.is_empty() {
                            ui.add_space(8.0_f32);
                            let note_color = if app.github_auth.auth_verified {
                                Color32::from_rgb(74, 222, 128)
                            } else {
                                ACCENT_AMBER
                            };
                            ui.label(
                                RichText::new(&app.github_auth.auth_status_note)
                                    .monospace()
                                    .size(10.5_f32)
                                    .color(note_color),
                            );
                        }
                    });
            });
        });
}

fn render_media_pipeline_view(app: &mut ResolveSidecarApp, ui: &mut egui::Ui) {
    ScrollArea::vertical()
        .id_source("media_pipeline_scroll")
        .show(ui, |ui| {
            // Card A: Cloud Storage & Mount
            surface_card()
                .show(ui, |ui| {
                    ui.label(
                        RichText::new("CARD A: CLOUD STORAGE & MOUNT (RCLONE WEBDAV)")
                            .strong()
                            .size(11.5_f32)
                            .color(TEXT_SECONDARY),
                    );
                    ui.add_space(8.0_f32);

                    // Section 1: Google Drive Auth & Remote Name configuration
                    ui.label(
                        RichText::new("ACCOUNT AUTHORIZATION")
                            .strong()
                            .size(10.0_f32)
                            .color(TEXT_MUTED),
                    );
                    ui.add_space(4.0_f32);
                    ui.horizontal_wrapped(|ui| {
                        if app.rclone_remote_authenticated {
                            ui.label(
                                RichText::new(format!(
                                    "\u{25cf} Authenticated ({})",
                                    app.storage_remote_name.trim()
                                ))
                                .monospace()
                                .strong()
                                .size(11.5_f32)
                                .color(Color32::from_rgb(74, 222, 128)),
                            );
                        } else {
                            ui.label(
                                RichText::new("\u{25cb} Not Authenticated")
                                    .monospace()
                                    .strong()
                                    .size(11.5_f32)
                                    .color(ACCENT_AMBER),
                            );
                        }
                        ui.add_space(10.0_f32);
                        if render_google_sign_in_button(ui, !app.rclone_oauth_in_progress)
                            .clicked()
                        {
                            app.start_google_drive_oauth();
                        }

                        if app.rclone_oauth_in_progress {
                            ui.add_space(8.0_f32);
                            ui.spinner();
                            ui.label(
                                RichText::new("Waiting for browser authorization...")
                                    .size(10.5_f32)
                                    .color(ACCENT_AMBER),
                            );
                        }
                    });
                    ui.add_space(6.0_f32);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("Remote Name:").size(11.0_f32).color(TEXT_SECONDARY));
                        let remote_resp = ui.add(
                            egui::TextEdit::singleline(&mut app.storage_remote_name)
                                .desired_width(130.0_f32),
                        );
                        if remote_resp.changed() {
                            app.check_rclone_remote_auth();
                        }
                        ui.add_space(10.0_f32);
                        ui.label(
                            RichText::new("Cloud Subfolder Path:")
                                .size(11.0_f32)
                                .color(TEXT_SECONDARY),
                        );
                        ui.add(
                            egui::TextEdit::singleline(&mut app.storage_remote_subfolder)
                                .desired_width(200.0_f32),
                        );
                    });

                    ui.add_space(10.0_f32);
                    ui.separator();
                    ui.add_space(8.0_f32);

                    // Section 2: Virtual Mount controls
                    ui.label(
                        RichText::new("VIRTUAL MOUNT CONTROLS")
                            .strong()
                            .size(10.0_f32)
                            .color(TEXT_MUTED),
                    );
                    ui.add_space(4.0_f32);
                    ui.horizontal_wrapped(|ui| {
                        let (label, color) = if app.z_drive_connected {
                            ("\u{25cf} Z: Connected", Color32::from_rgb(74, 222, 128))
                        } else {
                            ("\u{25cf} Z: Disconnected", ACCENT_AMBER)
                        };
                        ui.label(RichText::new(label).monospace().strong().size(12.0_f32).color(color));
                        ui.add_space(12.0_f32);
                        ui.label(RichText::new("Drive:").size(11.0_f32).color(TEXT_SECONDARY));
                        egui::ComboBox::from_id_source("drive_letter_combo")
                            .selected_text(&app.storage_drive_letter)
                            .width(64.0_f32)
                            .show_ui(ui, |ui| {
                                for d in ["Z:", "Y:", "X:"] {
                                    ui.selectable_value(
                                        &mut app.storage_drive_letter,
                                        d.to_string(),
                                        d,
                                    );
                                }
                            });

                        let can_remount = app.rclone_remote_authenticated || app.z_drive_connected;
                        let remount_btn = egui::Button::new(
                            RichText::new("Remount")
                                .strong()
                                .size(11.0_f32)
                                .color(Color32::WHITE),
                        )
                        .fill(if can_remount {
                            ACCENT_CYAN_DARK
                        } else {
                            BG_ELEVATED
                        })
                        .rounding(Rounding::same(4.0_f32));
                        if ui.add_enabled(can_remount, remount_btn).clicked() {
                            app.remount_z_drive();
                        }

                        let disc_btn = egui::Button::new(
                            RichText::new("Disconnect")
                                .strong()
                                .size(11.0_f32)
                                .color(ACCENT_RED),
                        )
                        .fill(BG_ELEVATED)
                        .stroke(Stroke::new(1.0_f32, ACCENT_RED.linear_multiply(0.55_f32)))
                        .rounding(Rounding::same(4.0_f32));
                        if ui.add(disc_btn).clicked() {
                            app.disconnect_z_drive();
                        }
                    });
                });

            ui.add_space(10.0_f32);

            // Card B: Folder Setup & Scripts
            surface_card()
                .show(ui, |ui| {
                    ui.label(
                        RichText::new("CARD B: FOLDER SETUP & RESOLVE SCRIPTS")
                            .strong()
                            .size(11.5_f32)
                            .color(TEXT_SECONDARY),
                    );
                    ui.add_space(8.0_f32);

                    let folder_checks = app.verify_pipeline_folders();
                    ui.horizontal_wrapped(|ui| {
                        for (path, ok) in folder_checks {
                            let (badge, color) = if ok {
                                (format!("\u{2713} {path}"), Color32::from_rgb(74, 222, 128))
                            } else {
                                (format!("\u{26a0} {path}"), ACCENT_AMBER)
                            };
                            Frame::none()
                                .fill(BG_ELEVATED)
                                .rounding(Rounding::same(4.0_f32))
                                .stroke(Stroke::new(1.0_f32, color.linear_multiply(0.45_f32)))
                                .inner_margin(Margin::symmetric(8.0_f32, 4.0_f32))
                                .show(ui, |ui| {
                                    ui.label(RichText::new(badge).monospace().size(10.5_f32).color(color));
                                });
                        }
                    });
                    ui.add_space(10.0_f32);

                    ui.horizontal_wrapped(|ui| {
                        let repair_btn = egui::Button::new(
                            RichText::new("Scaffold / Repair Structure")
                                .strong()
                                .size(11.0_f32)
                                .color(Color32::WHITE),
                        )
                        .fill(ACCENT_CYAN_DARK)
                        .rounding(Rounding::same(4.0_f32));
                        if ui.add(repair_btn).clicked() {
                            app.scaffold_repair_pipeline_folders();
                        }

                        let deploy_btn = egui::Button::new(
                            RichText::new("Sync Resolve Scripts Now")
                                .strong()
                                .size(11.0_f32)
                                .color(Color32::WHITE),
                        )
                        .fill(ACCENT_TEAL_BTN)
                        .rounding(Rounding::same(4.0_f32));
                        if ui.add(deploy_btn).clicked() {
                            app.redeploy_resolve_scripts();
                        }
                    });
                });

            ui.add_space(10.0_f32);

            // Card C: Ingest & Hardware Transcode Queue
            surface_card()
                .show(ui, |ui| {
                    ui.label(
                        RichText::new("CARD C: INGEST & HARDWARE TRANSCODE QUEUE")
                            .strong()
                            .size(11.5_f32)
                            .color(TEXT_SECONDARY),
                    );
                    ui.add_space(8.0_f32);

                    ui.horizontal_wrapped(|ui| {
                        let browse_btn = egui::Button::new(
                            RichText::new("\u{1f4c1} Browse Clips to Ingest...")
                                .strong()
                                .size(11.5_f32)
                                .color(Color32::WHITE),
                        )
                        .fill(ACCENT_CYAN_DARK)
                        .rounding(Rounding::same(4.0_f32));
                        if ui.add(browse_btn).clicked() {
                            app.browse_and_stage_clips();
                        }

                        ui.add_space(8.0_f32);
                        ui.label(
                            RichText::new(format!("Watch Folder: {}", app.ingest_watch_folder))
                                .monospace()
                                .size(10.5_f32)
                                .color(TEXT_MUTED),
                        );

                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let worker_running = app.is_transcode_worker_running();
                            let worker_btn = egui::Button::new(
                                RichText::new(if worker_running {
                                    "Transcode Worker Running"
                                } else {
                                    "Launch Transcode Worker"
                                })
                                .strong()
                                .size(11.0_f32)
                                .color(Color32::WHITE),
                            )
                            .fill(if worker_running {
                                ACCENT_TEAL_BTN
                            } else {
                                ACCENT_AMBER
                            })
                            .rounding(Rounding::same(4.0_f32));
                            if ui.add(worker_btn).clicked() {
                                app.launch_transcode_worker();
                            }
                        });
                    });

                    ui.add_space(10.0_f32);
                    ui.label(
                        RichText::new(format!("STAGING & TRANSCODE QUEUE ({})", app.ingest_queue.len()))
                            .strong()
                            .size(10.5_f32)
                            .color(TEXT_SECONDARY),
                    );
                    ui.add_space(6.0_f32);

                    if app.ingest_queue.is_empty() {
                        ui.label(
                            RichText::new("No clips queued yet. Click \"Browse Clips to Ingest...\" to stage footage.")
                                .size(11.0_f32)
                                .color(TEXT_MUTED),
                        );
                    } else {
                        egui::Grid::new("ingest_queue_grid")
                            .num_columns(4)
                            .striped(true)
                            .spacing(Vec2::new(12.0_f32, 6.0_f32))
                            .show(ui, |ui| {
                                ui.label(RichText::new("File").strong().size(10.5_f32).color(TEXT_MUTED));
                                ui.label(RichText::new("Target Profile").strong().size(10.5_f32).color(TEXT_MUTED));
                                ui.label(RichText::new("Destination").strong().size(10.5_f32).color(TEXT_MUTED));
                                ui.label(RichText::new("Status").strong().size(10.5_f32).color(TEXT_MUTED));
                                ui.end_row();
                                for job in &app.ingest_queue {
                                    ui.label(RichText::new(&job.filename).size(11.0_f32).color(TEXT_PRIMARY));
                                    ui.label(RichText::new(&job.profile).monospace().size(10.5_f32).color(ACCENT_CYAN));
                                    ui.label(RichText::new(&job.target_path).monospace().size(10.0_f32).color(TEXT_SECONDARY));
                                    let (st_text, st_color) = match &job.status {
                                        crate::app::IngestStatus::Queued => ("Queued".to_string(), ACCENT_AMBER),
                                        crate::app::IngestStatus::Staged => ("Staged".to_string(), ACCENT_TEAL),
                                        crate::app::IngestStatus::Error(e) => (format!("Error: {e}"), ACCENT_RED),
                                    };
                                    ui.label(RichText::new(st_text).strong().size(10.5_f32).color(st_color));
                                    ui.end_row();
                                }
                            });
                    }
                });
        });
}

fn render_first_run_setup_card(app: &mut ResolveSidecarApp, ui: &mut egui::Ui) {
    ui.vertical_centered(|ui| {
        ui.add_space((ui.available_height() * 0.18_f32).max(24.0_f32));
        Frame::none()
            .fill(BG_SURFACE)
            .rounding(Rounding::same(8.0_f32))
            .stroke(Stroke::new(1.0_f32, BORDER_SUBTLE))
            .inner_margin(Margin::same(20.0_f32))
            .show(ui, |ui| {
                ui.set_max_width(480.0_f32);
                ui.label(
                    RichText::new("Connect to Collaborative Timeline Repo")
                        .strong()
                        .size(15.0_f32)
                        .color(TEXT_PRIMARY),
                );
                ui.add_space(6.0_f32);
                ui.label(
                    RichText::new("Clone the shared timeline repository to start editing.")
                        .size(11.5_f32)
                        .color(TEXT_SECONDARY),
                );
                ui.add_space(14.0_f32);

                ui.label(
                    RichText::new("Git Remote URL")
                        .strong()
                        .size(11.0_f32)
                        .color(TEXT_SECONDARY),
                );
                ui.add_space(4.0_f32);
                ui.add(
                    egui::TextEdit::singleline(&mut app.setup_remote_url)
                        .desired_width(440.0_f32),
                );
                ui.add_space(10.0_f32);

                ui.label(
                    RichText::new("Editor Handle")
                        .strong()
                        .size(11.0_f32)
                        .color(TEXT_SECONDARY),
                );
                ui.add_space(4.0_f32);
                ui.add(
                    egui::TextEdit::singleline(&mut app.setup_editor_handle)
                        .desired_width(240.0_f32),
                );
                ui.add_space(16.0_f32);

                let clone_btn = egui::Button::new(
                    RichText::new("Clone & Start Editing")
                        .strong()
                        .size(12.0_f32)
                        .color(Color32::WHITE),
                )
                .fill(ACCENT_TEAL_BTN)
                .rounding(Rounding::same(5.0_f32));
                if ui.add(clone_btn).clicked() {
                    app.clone_and_start_editing();
                }
            });
    });
}

fn render_workspace_view(app: &mut ResolveSidecarApp, ui: &mut egui::Ui) {
    let total_w = ui.available_width();
    let total_h = ui.available_height();
    ui.set_max_width(total_w);

    ui.columns(2, |columns| {
        // Column 0 (Left): Width clamped to ui.available_width() * 0.45
        let left_w = (total_w * 0.45_f32).min(columns[0].available_width());
        columns[0].set_max_width(left_w);
        render_dag_column(app, &mut columns[0], left_w, total_h);

        // Column 1 (Right): Width clamped to ui.available_width() * 0.55
        let right_w = (total_w * 0.55_f32).min(columns[1].available_width());
        columns[1].set_max_width(right_w);
        render_inspector_column(app, &mut columns[1], right_w, total_h);
    });
}

fn render_dag_column(app: &mut ResolveSidecarApp, ui: &mut egui::Ui, width: f32, height: f32) {
    let avail_w = ui.available_width().min(width);
    ui.set_max_width(avail_w);
    Frame::none()
        .fill(BG_SURFACE)
        .rounding(Rounding::same(6.0_f32))
        .stroke(Stroke::new(1.0_f32, BORDER_SUBTLE))
        .inner_margin(Margin::same(12.0_f32))
        .show(ui, |ui| {
            let avail_w = ui.available_width().min((width - 24.0_f32).max(160.0_f32));
            ui.set_max_width(avail_w);
            ui.set_min_height((height - 24.0_f32).max(180.0_f32));

            let groups = build_commit_groups(&app.commits, &app.expanded_group_ids);
            let collapsed_group_count = groups
                .iter()
                .filter(|g| g.commits.len() > 1 && !g.is_expanded)
                .count();

            ui.horizontal(|ui| {
                ui.set_max_width(avail_w);
                ui.label(
                    RichText::new("TIMELINE DAG")
                        .strong()
                        .size(11.0_f32)
                        .color(TEXT_SECONDARY),
                );
                let count_label = if collapsed_group_count > 0 {
                    format!(
                        "({} rows / {} commits)",
                        groups
                            .iter()
                            .map(|g| if g.is_expanded { g.commits.len() } else { 1 })
                            .sum::<usize>(),
                        app.commits.len()
                    )
                } else {
                    format!("({} commits)", app.commits.len())
                };
                ui.label(
                    RichText::new(count_label)
                        .monospace()
                        .size(10.5_f32)
                        .color(TEXT_MUTED),
                );

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    match app.remote_sync_state {
                        crate::git_engine::RemoteSyncState::Behind(n) => {
                            let pull_btn = egui::Button::new(
                                RichText::new("Pull Latest Changes")
                                    .strong()
                                    .size(10.0_f32)
                                    .color(Color32::WHITE),
                            )
                            .fill(ACCENT_CYAN_DARK)
                            .rounding(Rounding::same(4.0_f32));
                            if ui.add(pull_btn).clicked() {
                                app.pull_latest_from_origin();
                            }
                            ui.label(
                                RichText::new(format!("Behind ({n})"))
                                    .monospace()
                                    .size(10.0_f32)
                                    .color(ACCENT_AMBER),
                            );
                        }
                        crate::git_engine::RemoteSyncState::Synced => {
                            ui.label(
                                RichText::new("\u{25cf} Remote: Synced")
                                    .monospace()
                                    .size(10.0_f32)
                                    .color(Color32::from_rgb(34, 197, 94)),
                            );
                        }
                        crate::git_engine::RemoteSyncState::Checking => {
                            ui.label(
                                RichText::new("\u{25cb} Remote: Checking...")
                                    .monospace()
                                    .size(10.0_f32)
                                    .color(TEXT_MUTED),
                            );
                        }
                    }
                });
            });
            ui.add_space(6.0_f32);

            if app.commits.is_empty() {
                ui.label(
                    RichText::new("No milestone commits yet.")
                        .size(12.0_f32)
                        .color(TEXT_SECONDARY),
                );
                return;
            }

            #[derive(Clone)]
            enum DagRow<'a> {
                Single(&'a CommitNodeInfo),
                CollapsedBurst(&'a CommitGroup),
                ExpandedMember {
                    commit: &'a CommitNodeInfo,
                    anchor_id: String,
                    is_first: bool,
                    is_last: bool,
                    group_len: usize,
                    primary_category: &'a str,
                },
            }

            // Map every raw commit OID to the visible node OID representing it
            let mut oid_to_visible: HashMap<&str, &str> = HashMap::with_capacity(app.commits.len());
            for g in &groups {
                if g.commits.len() > 1 && !g.is_expanded {
                    for c in &g.commits {
                        oid_to_visible.insert(c.full_id.as_str(), g.id.as_str());
                    }
                } else {
                    for c in &g.commits {
                        oid_to_visible.insert(c.full_id.as_str(), c.full_id.as_str());
                    }
                }
            }

            let mut visible_rows: Vec<DagRow<'_>> = Vec::new();
            let mut graph_inputs: Vec<graph_draw::GraphCommitInput> = Vec::new();

            for g in &groups {
                if g.commits.len() > 1 && !g.is_expanded {
                    let newest = &g.commits[0];
                    let oldest = &g.commits[g.commits.len() - 1];
                    let mut remapped_parents: Vec<String> = Vec::new();
                    for pid in &oldest.parent_ids {
                        let mapped = oid_to_visible
                            .get(pid.as_str())
                            .copied()
                            .unwrap_or(pid.as_str());
                        if mapped != g.id.as_str() && !remapped_parents.iter().any(|p| p == mapped) {
                            remapped_parents.push(mapped.to_string());
                        }
                    }
                    graph_inputs.push(graph_draw::GraphCommitInput {
                        full_id: g.id.clone(),
                        parent_ids: remapped_parents,
                        lane: newest.lane,
                        branch_kind: newest.branch_kind,
                        is_merge: newest.is_merge,
                    });
                    visible_rows.push(DagRow::CollapsedBurst(g));
                } else if g.commits.len() > 1 {
                    let anchor_id = burst_anchor_id(&g.commits);
                    let group_len = g.commits.len();
                    for (idx, c) in g.commits.iter().enumerate() {
                        let mut remapped_parents: Vec<String> = Vec::new();
                        for pid in &c.parent_ids {
                            let mapped = oid_to_visible
                                .get(pid.as_str())
                                .copied()
                                .unwrap_or(pid.as_str());
                            if mapped != c.full_id.as_str()
                                && !remapped_parents.iter().any(|p| p == mapped)
                            {
                                remapped_parents.push(mapped.to_string());
                            }
                        }
                        graph_inputs.push(graph_draw::GraphCommitInput {
                            full_id: c.full_id.clone(),
                            parent_ids: remapped_parents,
                            lane: c.lane,
                            branch_kind: c.branch_kind,
                            is_merge: c.is_merge,
                        });
                        visible_rows.push(DagRow::ExpandedMember {
                            commit: c,
                            anchor_id: anchor_id.clone(),
                            is_first: idx == 0,
                            is_last: idx + 1 == group_len,
                            group_len,
                            primary_category: g.primary_category.as_str(),
                        });
                    }
                } else {
                    let c = &g.commits[0];
                    let mut remapped_parents: Vec<String> = Vec::new();
                    for pid in &c.parent_ids {
                        let mapped = oid_to_visible
                            .get(pid.as_str())
                            .copied()
                            .unwrap_or(pid.as_str());
                        if mapped != c.full_id.as_str()
                            && !remapped_parents.iter().any(|p| p == mapped)
                        {
                            remapped_parents.push(mapped.to_string());
                        }
                    }
                    graph_inputs.push(graph_draw::GraphCommitInput {
                        full_id: c.full_id.clone(),
                        parent_ids: remapped_parents,
                        lane: c.lane,
                        branch_kind: c.branch_kind,
                        is_merge: c.is_merge,
                    });
                    visible_rows.push(DagRow::Single(c));
                }
            }

            let layout = graph_draw::compute_dag_layout(&graph_inputs);
            let mut clicked_commit_oid: Option<String> = None;
            let mut clicked_burst: Option<(String, String)> = None;
            let mut toggle_expand_key: Option<String> = None;
            let visible_selected_oid = app
                .selected_commit_oid
                .as_deref()
                .and_then(|sel| oid_to_visible.get(sel).copied().or(Some(sel)));

            ScrollArea::vertical()
                .id_source("dag_scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let scroll_w = ui.available_width().min(avail_w);
                    ui.set_max_width(scroll_w);
                    let row_text_w = (scroll_w - layout.gutter_width - 14.0_f32).max(100.0_f32);

                    ui.spacing_mut().item_spacing.y = 0.0_f32;
                    ui.horizontal_top(|ui| {
                        if let Some(oid) = graph_draw::paint_dag(
                            ui,
                            &layout,
                            &graph_inputs,
                            visible_selected_oid,
                        ) {
                            if let Some(group) = groups
                                .iter()
                                .find(|g| g.commits.len() > 1 && !g.is_expanded && g.id == oid)
                            {
                                let oldest = &group.commits[group.commits.len() - 1];
                                clicked_burst =
                                    Some((group.id.clone(), oldest.full_id.clone()));
                            } else {
                                clicked_commit_oid = Some(oid);
                            }
                        }

                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 0.0_f32;
                            for row in &visible_rows {
                                let (c, anchor_opt, badge_info) = match row {
                                    DagRow::Single(c) => (*c, None, None),
                                    DagRow::CollapsedBurst(g) => (
                                        &g.commits[0],
                                        Some(burst_anchor_id(&g.commits)),
                                        Some((true, g.commits.len(), false, false, g.primary_category.as_str())),
                                    ),
                                    DagRow::ExpandedMember {
                                        commit,
                                        anchor_id,
                                        is_first,
                                        is_last,
                                        group_len,
                                        primary_category,
                                    } => (
                                        *commit,
                                        Some(anchor_id.clone()),
                                        Some((false, *group_len, *is_first, *is_last, *primary_category)),
                                    ),
                                };

                                let is_selected = match row {
                                    DagRow::CollapsedBurst(g) => g.commits.iter().any(|gc| {
                                        app.selected_commit_oid.as_deref()
                                            == Some(gc.full_id.as_str())
                                    }),
                                    _ => {
                                        app.selected_commit_oid.as_deref() == Some(c.full_id.as_str())
                                    }
                                };

                                let (rect, response) = ui.allocate_exact_size(
                                    Vec2::new(row_text_w, graph_draw::ROW_HEIGHT),
                                    egui::Sense::click(),
                                );
                                let response =
                                    response.on_hover_cursor(egui::CursorIcon::PointingHand);

                                if is_selected {
                                    ui.painter().rect(
                                        rect,
                                        Rounding::same(4.0_f32),
                                        Color32::from_rgba_unmultiplied(56, 189, 248, 22),
                                        Stroke::new(
                                            1.0_f32,
                                            ACCENT_CYAN.linear_multiply(0.65_f32),
                                        ),
                                    );
                                } else if response.hovered() {
                                    ui.painter().rect_filled(
                                        rect,
                                        Rounding::same(4.0_f32),
                                        Color32::from_rgba_unmultiplied(255, 255, 255, 10),
                                    );
                                }

                                let mut toggle_clicked_here = false;
                                let inner_rect = rect.shrink2(Vec2::new(6.0_f32, 0.0_f32));
                                ui.allocate_ui_at_rect(inner_rect, |ui| {
                                    ui.horizontal_centered(|ui| {
                                        if let (
                                            Some(anchor_id),
                                            Some((is_collapsed, count, is_first, is_last, _cat)),
                                        ) = (anchor_opt.as_ref(), badge_info)
                                        {
                                            if is_collapsed || is_first {
                                                let arrow = if is_collapsed { "\u{25b8}" } else { "\u{25be}" };
                                                let badge_color = match c.branch_kind {
                                                    CommitBranchKind::Main => ACCENT_CYAN,
                                                    CommitBranchKind::Pass => ACCENT_TEAL,
                                                    CommitBranchKind::Experiment => COLOR_EXPERIMENT,
                                                };
                                                let toggle_btn = egui::Button::new(
                                                    RichText::new(format!("{arrow} {count}"))
                                                        .monospace()
                                                        .strong()
                                                        .size(9.5_f32)
                                                        .color(badge_color),
                                                )
                                                .fill(BG_ELEVATED)
                                                .stroke(Stroke::new(
                                                    1.0_f32,
                                                    badge_color.linear_multiply(0.55_f32),
                                                ))
                                                .rounding(Rounding::same(3.0_f32));
                                                if ui.add(toggle_btn).clicked() {
                                                    toggle_expand_key = Some(anchor_id.clone());
                                                    toggle_clicked_here = true;
                                                }
                                            } else {
                                                let tree_glyph = if is_last { "\u{2514}" } else { "\u{251c}" };
                                                ui.label(
                                                    RichText::new(tree_glyph)
                                                        .monospace()
                                                        .size(10.5_f32)
                                                        .color(TEXT_MUTED),
                                                );
                                            }
                                        }

                                        let hash_text = match row {
                                            DagRow::CollapsedBurst(g) => g.representative_hash.as_str(),
                                            _ => c.short_id.as_str(),
                                        };
                                        ui.label(
                                            RichText::new(hash_text)
                                                .monospace()
                                                .size(10.5_f32)
                                                .color(TEXT_MUTED),
                                        );

                                        if let Some(tag) = &c.tag {
                                            let tag_color = if c.lane >= 2 {
                                                COLOR_EXPERIMENT
                                            } else {
                                                match c.branch_kind {
                                                    CommitBranchKind::Main => ACCENT_CYAN,
                                                    CommitBranchKind::Pass => ACCENT_TEAL,
                                                    CommitBranchKind::Experiment => COLOR_EXPERIMENT,
                                                }
                                            };
                                            ui.allocate_ui(
                                                Vec2::new(72.0_f32, 18.0_f32),
                                                |ui| {
                                                    ui.add(
                                                        Label::new(
                                                            RichText::new(tag)
                                                                .monospace()
                                                                .size(10.0_f32)
                                                                .color(tag_color),
                                                        )
                                                        .truncate(true),
                                                    );
                                                },
                                            );
                                        }

                                        let display_msg = match row {
                                            DagRow::CollapsedBurst(g) => format_group_header(g),
                                            _ => clean_leaf_commit_summary(&c.message),
                                        };

                                        let time_label = match row {
                                            DagRow::CollapsedBurst(_) => {
                                                format!("\u{2022} {}", c.time)
                                            }
                                            _ => c.time.clone(),
                                        };

                                        let summary_w =
                                            (ui.available_width() - 62.0_f32).max(40.0_f32);
                                        ui.allocate_ui(
                                            Vec2::new(summary_w, 18.0_f32),
                                            |ui| {
                                                ui.add(
                                                    Label::new(
                                                        RichText::new(&display_msg)
                                                            .size(11.5_f32)
                                                            .color(TEXT_PRIMARY),
                                                    )
                                                    .truncate(true),
                                                );
                                            },
                                        );

                                        ui.label(
                                            RichText::new(&time_label)
                                                .monospace()
                                                .size(10.0_f32)
                                                .color(TEXT_MUTED),
                                        );
                                    });
                                });

                                let response = match row {
                                    DagRow::CollapsedBurst(g) => {
                                        let span = (g.end_time - g.start_time).max(0);
                                        response.on_hover_text(format!(
                                            "{} \u{2022} {} changes over {}s (click badge to inspect individual cuts)",
                                            g.branch,
                                            g.commits.len(),
                                            span
                                        ))
                                    }
                                    _ => response,
                                };

                                if response.clicked() && !toggle_clicked_here {
                                    match row {
                                        DagRow::CollapsedBurst(g) => {
                                            let oldest = &g.commits[g.commits.len() - 1];
                                            clicked_burst =
                                                Some((g.id.clone(), oldest.full_id.clone()));
                                        }
                                        _ => {
                                            clicked_commit_oid = Some(c.full_id.clone());
                                        }
                                    }
                                }
                            }
                        });
                    });
                });

            if let Some(key) = toggle_expand_key {
                app.toggle_group_expanded(&key);
            } else if let Some((newest, oldest)) = clicked_burst {
                app.select_burst_group(newest, oldest);
            } else if let Some(oid) = clicked_commit_oid {
                app.select_commit(oid);
            }
        });
}

fn render_inspector_column(
    app: &mut ResolveSidecarApp,
    ui: &mut egui::Ui,
    width: f32,
    height: f32,
) {
    let avail_w = ui.available_width().min(width);
    ui.set_max_width(avail_w);
    Frame::none()
        .fill(BG_SURFACE)
        .rounding(Rounding::same(6.0_f32))
        .stroke(Stroke::new(1.0_f32, BORDER_SUBTLE))
        .inner_margin(Margin::same(12.0_f32))
        .show(ui, |ui| {
            let avail_w = ui.available_width().min((width - 24.0_f32).max(160.0_f32));
            ui.set_max_width(avail_w);
            ui.set_min_height((height - 24.0_f32).max(180.0_f32));

            ScrollArea::vertical()
                .id_source("inspector_scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let content_w = ui.available_width().min(avail_w);
                    ui.set_max_width(content_w);

                    if app.show_tickets_drawer {
                        render_tickets_drawer(app, ui, content_w);
                        return;
                    }

                    if let Some(pr_branch) = app.pr_review_branch.clone() {
                        render_pr_review_drawer(app, ui, &pr_branch, content_w);
                        return;
                    }

                    ui.label(
                        RichText::new("COMMIT INSPECTOR")
                            .strong()
                            .size(11.0_f32)
                            .color(TEXT_SECONDARY),
                    );
                    ui.add_space(8.0_f32);

                    let selected = app.selected_commit_oid.as_ref().and_then(|sel| {
                        app.commits.iter().find(|c| &c.full_id == sel).cloned()
                    });

                    let Some(commit) = selected else {
                        ui.label(
                            RichText::new("Select a commit in the DAG to inspect deltas.")
                                .size(12.0_f32)
                                .color(TEXT_MUTED),
                        );
                        return;
                    };

                    // Commit Details Card
                    Frame::none()
                        .fill(BG_ELEVATED)
                        .rounding(Rounding::same(5.0_f32))
                        .stroke(Stroke::new(1.0_f32, BORDER_SUBTLE))
                        .inner_margin(Margin::same(10.0_f32))
                        .show(ui, |ui| {
                            let cw = ui.available_width().min((content_w - 20.0_f32).max(120.0_f32));
                            ui.set_max_width(cw);
                            ui.horizontal_wrapped(|ui| {
                                ui.label(
                                    RichText::new(&commit.short_id)
                                        .monospace()
                                        .strong()
                                        .size(12.0_f32)
                                        .color(ACCENT_CYAN),
                                );
                                ui.label(
                                    RichText::new(&commit.author)
                                        .size(11.0_f32)
                                        .color(TEXT_PRIMARY),
                                );
                                ui.label(
                                    RichText::new(format!("• {}", commit.time))
                                        .monospace()
                                        .size(11.0_f32)
                                        .color(TEXT_MUTED),
                                );
                            });
                            ui.add_space(4.0_f32);
                            ui.add(
                                Label::new(
                                    RichText::new(&commit.message)
                                        .size(12.5_f32)
                                        .color(TEXT_PRIMARY),
                                )
                                .wrap(true),
                            );
                            if let Some((newest, oldest)) = &app.selected_burst_range {
                                ui.add_space(4.0_f32);
                                ui.label(
                                    RichText::new(format!(
                                        "Cumulative Burst Delta ({} .. {})",
                                        &oldest[..7.min(oldest.len())],
                                        &newest[..7.min(newest.len())]
                                    ))
                                    .monospace()
                                    .size(10.5_f32)
                                    .color(ACCENT_AMBER),
                                );
                            }
                            if let Some(tag) = &commit.tag {
                                ui.add_space(4.0_f32);
                                ui.add(
                                    Label::new(
                                        RichText::new(format!("Branch: {tag}"))
                                            .monospace()
                                            .size(10.5_f32)
                                            .color(TEXT_SECONDARY),
                                    )
                                    .truncate(true),
                                );
                            }
                        });

                    ui.add_space(10.0_f32);

                    // Action Buttons
                    ui.horizontal_wrapped(|ui| {
                        let restore_btn = egui::Button::new(
                            RichText::new("Restore to Tab in Resolve")
                                .strong()
                                .size(11.0_f32)
                                .color(Color32::WHITE),
                        )
                        .fill(ACCENT_CYAN_DARK)
                        .rounding(Rounding::same(5.0_f32));
                        if ui.add(restore_btn).clicked() {
                            trigger_restore_to_resolve(app, &commit.full_id);
                        }
                    });

                    if let Some(branch) = branch_for_commit(app, &commit) {
                        ui.add_space(10.0_f32);
                        Frame::none()
                            .fill(BG_ELEVATED)
                            .rounding(Rounding::same(5.0_f32))
                            .stroke(Stroke::new(
                                1.0_f32,
                                ACCENT_AMBER.linear_multiply(0.65_f32),
                            ))
                            .inner_margin(Margin::same(10.0_f32))
                            .show(ui, |ui| {
                                let cw = ui
                                    .available_width()
                                    .min((content_w - 20.0_f32).max(120.0_f32));
                                ui.set_max_width(cw);
                                ui.add(
                                    Label::new(
                                        RichText::new(format!("PASS / PR: main \u{2190} {branch}"))
                                            .monospace()
                                            .strong()
                                            .size(11.0_f32)
                                            .color(ACCENT_AMBER),
                                    )
                                    .truncate(true),
                                );
                                ui.add_space(6.0_f32);
                                ui.horizontal_wrapped(|ui| {
                                    let review_btn = egui::Button::new(
                                        RichText::new("Review PR Details")
                                            .strong()
                                            .size(11.0_f32)
                                            .color(ACCENT_AMBER),
                                    )
                                    .fill(BG_SURFACE)
                                    .stroke(Stroke::new(
                                        1.0_f32,
                                        ACCENT_AMBER.linear_multiply(0.65_f32),
                                    ))
                                    .rounding(Rounding::same(5.0_f32));
                                    if ui.add(review_btn).clicked() {
                                        app.open_pr_review(&branch);
                                    }

                                    let merge_btn = egui::Button::new(
                                        RichText::new("Accept & Merge to Main")
                                            .strong()
                                            .size(11.0_f32)
                                            .color(Color32::WHITE),
                                    )
                                    .fill(ACCENT_TEAL_BTN)
                                    .rounding(Rounding::same(5.0_f32));
                                    if ui.add(merge_btn).clicked() {
                                        app.merge_branch_to_main(&branch);
                                    }

                                    let discard_btn = egui::Button::new(
                                        RichText::new("Discard Pass")
                                            .strong()
                                            .size(11.0_f32)
                                            .color(ACCENT_RED),
                                    )
                                    .fill(BG_SURFACE)
                                    .stroke(Stroke::new(
                                        1.0_f32,
                                        ACCENT_RED.linear_multiply(0.55_f32),
                                    ))
                                    .rounding(Rounding::same(5.0_f32));
                                    if ui.add(discard_btn).clicked() {
                                        app.discard_sandbox_branch(&branch);
                                    }
                                });
                            });
                    }

                    ui.add_space(12.0_f32);
                    ui.separator();
                    ui.add_space(8.0_f32);

                    // Timeline Delta Grid
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("TIMELINE CHANGES")
                                .strong()
                                .size(10.5_f32)
                                .color(TEXT_SECONDARY),
                        );
                        ui.label(
                            RichText::new(format!("({} deltas)", app.selected_commit_changes.len()))
                                .monospace()
                                .size(10.5_f32)
                                .color(TEXT_MUTED),
                        );
                    });
                    ui.add_space(6.0_f32);

                    if app.selected_commit_changes.is_empty() {
                        ui.label(
                            RichText::new("No clip or track deltas detected for this commit.")
                                .size(11.0_f32)
                                .color(TEXT_MUTED),
                        );
                        return;
                    }

                    if let Some(local_path) = render_delta_grid(
                        ui,
                        "timeline_delta_grid",
                        &app.selected_commit_changes,
                        content_w,
                    ) {
                        app.consolidate_media_to_z(&local_path);
                    }
                });
        });
}

fn render_pr_review_drawer(
    app: &mut ResolveSidecarApp,
    ui: &mut egui::Ui,
    pr_branch: &str,
    content_w: f32,
) {
    ui.set_max_width(content_w);
    Frame::none()
        .fill(BG_ELEVATED)
        .rounding(Rounding::same(6.0_f32))
        .stroke(Stroke::new(
            1.0_f32,
            ACCENT_AMBER.linear_multiply(0.65_f32),
        ))
        .inner_margin(Margin::same(10.0_f32))
        .show(ui, |ui| {
            let cw = ui
                .available_width()
                .min((content_w - 20.0_f32).max(120.0_f32));
            ui.set_max_width(cw);

            ui.add(
                Label::new(
                    RichText::new(format!("Pull Request: {pr_branch} \u{2192} main"))
                        .monospace()
                        .strong()
                        .size(12.0_f32)
                        .color(ACCENT_AMBER),
                )
                .truncate(true),
            );
            let meta = app.pr_review_meta.clone();
            let commit_word = if meta.commit_count == 1 {
                "commit"
            } else {
                "commits"
            };
            ui.label(
                RichText::new(format!(
                    "by {} \u{2022} {} {commit_word}",
                    meta.author, meta.commit_count
                ))
                .size(10.5_f32)
                .color(TEXT_SECONDARY),
            );
            ui.add_space(6.0_f32);

            let dur_sign = if meta.duration_delta_sec > 0.05 {
                "+"
            } else {
                ""
            };
            let dur_str = format!("{dur_sign}{:.1}s", meta.duration_delta_sec);
            let total_clip_deltas = app.pr_review_changes.len();
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(format!("Net cuts: {}", meta.total_cuts))
                        .monospace()
                        .size(10.5_f32)
                        .color(ACCENT_CYAN),
                );
                ui.label(
                    RichText::new(format!("\u{2022} Audio adjustments: {}", meta.audio_edits))
                        .monospace()
                        .size(10.5_f32)
                        .color(ACCENT_TEAL),
                );
                ui.label(
                    RichText::new(format!("\u{2022} Total clip deltas: {total_clip_deltas}"))
                        .monospace()
                        .size(10.5_f32)
                        .color(TEXT_PRIMARY),
                );
                ui.label(
                    RichText::new(format!("\u{2022} Net duration: {dur_str}"))
                        .monospace()
                        .size(10.5_f32)
                        .color(TEXT_SECONDARY),
                );
            });
            ui.add_space(8.0_f32);

            ui.horizontal_wrapped(|ui| {
                let preview_btn = egui::Button::new(
                    RichText::new("Preview Cut in Resolve")
                        .strong()
                        .size(11.0_f32)
                        .color(Color32::WHITE),
                )
                .fill(ACCENT_CYAN_DARK)
                .rounding(Rounding::same(5.0_f32));
                if ui.add(preview_btn).clicked() {
                    app.preview_branch_cut_in_resolve(pr_branch);
                }

                let merge_btn = egui::Button::new(
                    RichText::new("Accept & Merge to Main")
                        .strong()
                        .size(11.0_f32)
                        .color(Color32::WHITE),
                )
                .fill(ACCENT_TEAL_BTN)
                .rounding(Rounding::same(5.0_f32));
                if ui.add(merge_btn).clicked() {
                    app.merge_branch_to_main(pr_branch);
                }

                let close_btn = egui::Button::new(
                    RichText::new("Close Review")
                        .strong()
                        .size(11.0_f32)
                        .color(TEXT_SECONDARY),
                )
                .fill(BG_SURFACE)
                .stroke(Stroke::new(1.0_f32, BORDER_SUBTLE))
                .rounding(Rounding::same(5.0_f32));
                if ui.add(close_btn).clicked() {
                    app.close_pr_review();
                }
            });
        });

    ui.add_space(10.0_f32);
    ui.label(
        RichText::new("CUMULATIVE BRANCH DELTA VS MAIN")
            .strong()
            .size(10.5_f32)
            .color(TEXT_SECONDARY),
    );
    ui.add_space(6.0_f32);

    if app.pr_review_changes.is_empty() {
        ui.label(
            RichText::new("No cumulative OTIO differences between this branch and main.")
                .size(11.0_f32)
                .color(TEXT_MUTED),
        );
    } else if let Some(local_path) = render_delta_grid(
        ui,
        "pr_review_delta_grid",
        &app.pr_review_changes,
        content_w,
    ) {
        app.consolidate_media_to_z(&local_path);
    }
}

fn render_tickets_drawer(app: &mut ResolveSidecarApp, ui: &mut egui::Ui, content_w: f32) {
    ui.set_max_width(content_w);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("\u{1f4ac} TICKETS & NITPICKS")
                .strong()
                .size(11.5_f32)
                .color(TEXT_PRIMARY),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let close_btn = egui::Button::new(
                RichText::new("Close")
                    .strong()
                    .size(10.5_f32)
                    .color(TEXT_SECONDARY),
            )
            .fill(BG_ELEVATED)
            .stroke(Stroke::new(1.0_f32, BORDER_SUBTLE))
            .rounding(Rounding::same(4.0_f32));
            if ui.add(close_btn).clicked() {
                app.show_tickets_drawer = false;
            }
        });
    });
    ui.add_space(8.0_f32);

    // New Ticket Form
    Frame::none()
        .fill(BG_ELEVATED)
        .rounding(Rounding::same(6.0_f32))
        .stroke(Stroke::new(1.0_f32, BORDER_SUBTLE))
        .inner_margin(Margin::same(10.0_f32))
        .show(ui, |ui| {
            let cw = ui
                .available_width()
                .min((content_w - 20.0_f32).max(120.0_f32));
            ui.set_max_width(cw);

            ui.label(
                RichText::new("NEW TICKET")
                    .strong()
                    .size(10.0_f32)
                    .color(TEXT_SECONDARY),
            );
            ui.add_space(6.0_f32);

            ui.horizontal_wrapped(|ui| {
                for cat in [
                    crate::tickets::TicketCategory::Nitpick,
                    crate::tickets::TicketCategory::FeatureIdea,
                    crate::tickets::TicketCategory::BugReport,
                ] {
                    let selected = app.new_ticket_category == cat;
                    let btn = egui::Button::new(
                        RichText::new(cat.label())
                            .strong()
                            .size(10.5_f32)
                            .color(if selected {
                                Color32::WHITE
                            } else {
                                TEXT_SECONDARY
                            }),
                    )
                    .fill(if selected { ACCENT_CYAN_DARK } else { BG_SURFACE })
                    .stroke(Stroke::new(1.0_f32, BORDER_SUBTLE))
                    .rounding(Rounding::same(4.0_f32));
                    if ui.add(btn).clicked() {
                        app.new_ticket_category = cat;
                    }
                }
            });
            ui.add_space(6.0_f32);

            ui.add(
                egui::TextEdit::singleline(&mut app.new_ticket_title)
                    .hint_text("Short ticket title...")
                    .desired_width(cw),
            );
            ui.add_space(6.0_f32);

            ui.add(
                egui::TextEdit::multiline(&mut app.new_ticket_description)
                    .hint_text("Describe the edit note, nitpick, or issue...")
                    .desired_rows(3)
                    .desired_width(cw),
            );
            ui.add_space(6.0_f32);

            let head_hash = app.active_head_short_hash();
            let tl_name = app.active_timeline_name();
            let ts_now = chrono::Utc::now().format("%Y-%m-%d %H:%M");
            ui.label(
                RichText::new(format!("HEAD: {head_hash} \u{2022} Timeline: {tl_name} \u{2022} {ts_now}"))
                    .monospace()
                    .size(9.5_f32)
                    .color(TEXT_MUTED),
            );
            ui.add_space(6.0_f32);

            let can_submit = !app.new_ticket_title.trim().is_empty();
            let submit_btn = egui::Button::new(
                RichText::new("Submit Ticket")
                    .strong()
                    .size(11.0_f32)
                    .color(Color32::WHITE),
            )
            .fill(if can_submit {
                ACCENT_TEAL_BTN
            } else {
                BG_SURFACE
            })
            .rounding(Rounding::same(5.0_f32));
            if ui.add_enabled(can_submit, submit_btn).clicked() {
                app.submit_new_ticket();
            }
        });

    ui.add_space(10.0_f32);
    ui.label(
        RichText::new(format!("TICKET LIST ({})", app.tickets.len()))
            .strong()
            .size(10.5_f32)
            .color(TEXT_SECONDARY),
    );
    ui.add_space(6.0_f32);

    if app.tickets.is_empty() {
        ui.label(
            RichText::new("No tickets submitted yet.")
                .size(11.0_f32)
                .color(TEXT_MUTED),
        );
        return;
    }

    let snapshot = app.tickets.clone();
    let mut toggle_id: Option<String> = None;
    for t in snapshot {
        let is_open = t.status == crate::tickets::TicketStatus::Open;
        let status_color = if is_open { ACCENT_AMBER } else { ACCENT_TEAL };
        Frame::none()
            .fill(BG_ELEVATED)
            .rounding(Rounding::same(5.0_f32))
            .stroke(Stroke::new(1.0_f32, BORDER_SUBTLE))
            .inner_margin(Margin::same(8.0_f32))
            .show(ui, |ui| {
                let cw = ui
                    .available_width()
                    .min((content_w - 16.0_f32).max(120.0_f32));
                ui.set_max_width(cw);

                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("[{}]", t.category.label()))
                            .monospace()
                            .strong()
                            .size(10.0_f32)
                            .color(ACCENT_CYAN),
                    );
                    ui.add(
                        Label::new(
                            RichText::new(&t.title)
                                .strong()
                                .size(11.5_f32)
                                .color(TEXT_PRIMARY),
                        )
                        .truncate(true),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let status_btn = egui::Button::new(
                            RichText::new(t.status.label())
                                .monospace()
                                .strong()
                                .size(9.5_f32)
                                .color(status_color),
                        )
                        .fill(BG_SURFACE)
                        .stroke(Stroke::new(1.0_f32, status_color.linear_multiply(0.6_f32)))
                        .rounding(Rounding::same(3.0_f32));
                        if ui.add(status_btn).clicked() {
                            toggle_id = Some(t.id.clone());
                        }
                    });
                });

                ui.label(
                    RichText::new(format!(
                        "{} \u{2022} {} \u{2022} {} ({})",
                        t.author, t.created_at, t.timeline_name, t.commit_hash
                    ))
                    .monospace()
                    .size(9.5_f32)
                    .color(TEXT_MUTED),
                );
                if !t.description.is_empty() {
                    ui.add_space(4.0_f32);
                    ui.label(
                        RichText::new(&t.description)
                            .size(11.0_f32)
                            .color(TEXT_SECONDARY),
                    );
                }
            });
        ui.add_space(6.0_f32);
    }

    if let Some(id) = toggle_id {
        app.toggle_ticket_status(&id);
    }
}

fn render_delta_grid(
    ui: &mut egui::Ui,
    id_source: &str,
    rows: &[crate::CommitChangeRow],
    content_w: f32,
) -> Option<String> {
    let mut consolidate_req: Option<String> = None;
    Frame::none()
        .fill(BG_ELEVATED)
        .rounding(Rounding::same(5.0_f32))
        .stroke(Stroke::new(1.0_f32, BORDER_SUBTLE))
        .inner_margin(Margin::same(8.0_f32))
        .show(ui, |ui| {
            let gw = ui
                .available_width()
                .min((content_w - 16.0_f32).max(120.0_f32));
            ui.set_max_width(gw);
            let col_media_w = (gw - 250.0_f32).max(90.0_f32);

            egui::Grid::new(id_source)
                .num_columns(4)
                .striped(true)
                .spacing(Vec2::new(8.0_f32, 5.0_f32))
                .show(ui, |ui| {
                    ui.label(RichText::new("Track").strong().size(10.0_f32).color(TEXT_MUTED));
                    ui.label(RichText::new("Media File").strong().size(10.0_f32).color(TEXT_MUTED));
                    ui.label(RichText::new("Action").strong().size(10.0_f32).color(TEXT_MUTED));
                    ui.label(RichText::new("Range / Delta").strong().size(10.0_f32).color(TEXT_MUTED));
                    ui.end_row();

                    for row in rows {
                        let tc = if row.is_audio { ACCENT_TEAL } else { ACCENT_CYAN };
                        let cc = match row.change_type.as_str() {
                            "Added" | "Track Added" => ACCENT_TEAL,
                            "Deleted" | "Track Removed" => ACCENT_RED,
                            "Trimmed" => ACCENT_AMBER,
                            "Split" => ACCENT_CYAN,
                            _ => TEXT_PRIMARY,
                        };
                        let missing_z = crate::is_z_drive_missing(row.media_path.as_deref());
                        let local_not_on_z =
                            crate::is_local_media_not_on_z(row.media_path.as_deref());

                        ui.label(
                            RichText::new(&row.track)
                                .monospace()
                                .strong()
                                .size(10.5_f32)
                                .color(tc),
                        );
                        ui.allocate_ui(Vec2::new(col_media_w, 20.0_f32), |ui| {
                            ui.vertical(|ui| {
                                ui.add(
                                    Label::new(
                                        RichText::new(&row.media_file)
                                            .size(11.0_f32)
                                            .color(TEXT_PRIMARY),
                                    )
                                    .truncate(true),
                                );
                                if local_not_on_z {
                                    ui.horizontal_wrapped(|ui| {
                                        let warn_resp = ui.label(
                                            RichText::new("\u{26a0} Local Media (Not on Z:\\)")
                                                .monospace()
                                                .strong()
                                                .size(9.5_f32)
                                                .color(ACCENT_AMBER),
                                        );
                                        if let Some(ref mp) = row.media_path {
                                            warn_resp.on_hover_text(mp);
                                        }
                                        let cons_btn = egui::Button::new(
                                            RichText::new("Move to Z:")
                                                .strong()
                                                .size(9.5_f32)
                                                .color(Color32::WHITE),
                                        )
                                        .fill(ACCENT_CYAN_DARK)
                                        .rounding(Rounding::same(3.0_f32));
                                        if ui.add(cons_btn).clicked() {
                                            consolidate_req = row.media_path.clone();
                                        }
                                    });
                                } else if missing_z {
                                    let warn_resp = ui.label(
                                        RichText::new("\u{26a0} Missing on Z:")
                                            .monospace()
                                            .strong()
                                            .size(9.5_f32)
                                            .color(ACCENT_AMBER),
                                    );
                                    if let Some(ref mp) = row.media_path {
                                        warn_resp.on_hover_text(mp);
                                    }
                                }
                            });
                        });
                        ui.label(
                            RichText::new(&row.change_type)
                                .strong()
                                .size(10.5_f32)
                                .color(cc),
                        );
                        ui.add(
                            Label::new(
                                RichText::new(&row.position)
                                    .monospace()
                                    .size(10.5_f32)
                                    .color(TEXT_SECONDARY),
                            )
                            .wrap(false),
                        );
                        ui.end_row();
                    }
                });
        });
    consolidate_req
}

fn find_sidecar_import_script() -> std::path::PathBuf {
    let local = std::path::PathBuf::from("sidecar_import.py");
    if local.is_file() {
        return local;
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let adjacent = dir.join("sidecar_import.py");
            if adjacent.is_file() {
                return adjacent;
            }
        }
    }
    if let Some(appdata) = dirs::data_dir() {
        let fusion_utility = appdata
            .join("Blackmagic Design")
            .join("DaVinci Resolve")
            .join("Support")
            .join("Fusion")
            .join("Scripts")
            .join("Utility")
            .join("sidecar_import.py");
        if fusion_utility.is_file() {
            return fusion_utility;
        }
    }
    local
}

pub(crate) fn trigger_restore_to_resolve(app: &mut ResolveSidecarApp, commit_hash: &str) {
    let restore_path = match crate::git_engine::export_commit_otio(&app.repo_path, commit_hash) {
        Ok(p) => p,
        Err(err) => {
            app.status_message = format!("Restore export error: {err}");
            return;
        }
    };

    let script_path = find_sidecar_import_script();
    let mut cmd = std::process::Command::new("python");
    cmd.arg(&script_path).arg(&restore_path);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }

    let hash = &commit_hash[..7.min(commit_hash.len())];
    match cmd.status() {
        Ok(status) if status.success() => {
            app.status_message = format!("Restored commit {hash} to Sidecar_Cut");
        }
        Ok(status) => {
            app.status_message = format!("Restore failed for commit {hash} (exit {status})");
        }
        Err(err) => {
            app.status_message = format!("Failed to execute python sidecar_import.py: {err}");
        }
    }
}

fn branch_for_commit(app: &ResolveSidecarApp, commit: &crate::GitCommitNode) -> Option<String> {
    if commit.branch_kind == CommitBranchKind::Main {
        return None;
    }
    if let Some(ref bname) = commit.branch_name {
        if bname != "main" && !bname.starts_with("scratch/") {
            return Some(bname.clone());
        }
    }
    if let Some(tag) = &commit.tag {
        for part in tag.split(',').map(str::trim) {
            if !part.is_empty()
                && part != "HEAD"
                && part != "main"
                && !part.starts_with("scratch/")
            {
                return Some(part.to_string());
            }
        }
    }
    if app.current_branch != "main" && !app.current_branch.starts_with("scratch/") {
        return Some(app.current_branch.clone());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_commit(
        id: &str,
        parent: Option<&str>,
        ts: i64,
        msg: &str,
        lane: usize,
        branch_name: Option<&str>,
    ) -> CommitNodeInfo {
        CommitNodeInfo {
            full_id: id.to_string(),
            short_id: id.chars().take(7).collect(),
            message: msg.to_string(),
            author: "Zane".to_string(),
            time: "just now".to_string(),
            timestamp_secs: ts,
            tag: None,
            parent_ids: parent.into_iter().map(str::to_string).collect(),
            lane,
            is_merge: false,
            branch_kind: if lane == 0 {
                CommitBranchKind::Main
            } else {
                CommitBranchKind::Pass
            },
            branch_name: branch_name.map(str::to_string),
            tracks: Vec::new(),
        }
    }

    #[test]
    fn build_commit_groups_aggregates_consecutive_burst_and_promotes_category() {
        let mut c4 = mk_commit("c4_0000", Some("c3_0000"), 1050, "[Audio] Added score.wav on A1", 0, None);
        c4.tracks = vec!["A1".to_string()];
        let mut c3 = mk_commit("c3_0000", Some("c2_0000"), 1020, "[Edit] Trimmed intro on V1", 0, None);
        c3.tracks = vec!["V1".to_string()];
        let mut c2 = mk_commit("c2_0000", Some("c1_0000"), 1000, "[Edit] Split clip on V1", 0, None);
        c2.tracks = vec!["V1".to_string()];
        let c1 = mk_commit("c1_0000", None, 500, "[Edit] Initial baseline", 0, None);
        let commits = vec![c4, c3, c2, c1];

        let expanded = HashSet::new();
        let groups = build_commit_groups(&commits, &expanded);
        assert_eq!(groups.len(), 2);

        assert_eq!(groups[0].id, "c4_0000");
        assert_eq!(groups[0].commits.len(), 3);
        assert_eq!(groups[0].start_time, 1000);
        assert_eq!(groups[0].end_time, 1050);
        assert_eq!(groups[0].primary_category, "[Cut & Mix]");
        assert_eq!(
            format_group_header(&groups[0]),
            "[Session: Edit & Mix] 3 changes (A1, V1)"
        );
        assert!(!groups[0].is_expanded);

        assert_eq!(groups[1].id, "c1_0000");
        assert_eq!(groups[1].commits.len(), 1);
        assert_eq!(
            clean_leaf_commit_summary("[Edit] Trimmed A001.mp4 on V1"),
            "Trimmed A001.mp4 on V1"
        );

        // Expanding by anchor (oldest in burst) sets is_expanded = true
        let mut exp2 = HashSet::new();
        exp2.insert("c2_0000".to_string());
        let groups_exp = build_commit_groups(&commits, &exp2);
        assert!(groups_exp[0].is_expanded);
    }
}
