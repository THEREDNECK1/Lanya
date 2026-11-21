use crate::{ResolveSidecarApp, StorageMountState};
use eframe::egui::{
    self, Color32, Frame, Margin, RichText, Rounding, ScrollArea, Stroke, Vec2,
};

pub fn render_app(app: &mut ResolveSidecarApp, ctx: &egui::Context) {
    let bg_base = Color32::from_rgb(15, 16, 20);
    let bg_sub = Color32::from_rgb(22, 24, 30);
    let bg_card = Color32::from_rgb(28, 31, 39);
    let border_subtle = Color32::from_rgb(42, 46, 58);
    let accent_green = Color32::from_rgb(16, 185, 129);
    let accent_blue = Color32::from_rgb(59, 130, 246);
    let accent_amber = Color32::from_rgb(245, 158, 11);
    let accent_red = Color32::from_rgb(239, 68, 68);
    let text_primary = Color32::from_rgb(243, 244, 246);
    let text_secondary = Color32::from_rgb(156, 163, 175);
    let text_muted = Color32::from_rgb(107, 114, 128);

    let mut visuals = egui::Visuals::dark();
    visuals.override_text_color = Some(text_primary);
    visuals.panel_fill = bg_base;
    visuals.window_fill = bg_sub;
    visuals.widgets.noninteractive.bg_fill = bg_sub;
    visuals.widgets.inactive.bg_fill = bg_card;
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(38, 42, 54);
    visuals.widgets.active.bg_fill = Color32::from_rgb(48, 54, 70);
    visuals.widgets.inactive.rounding = Rounding::same(5.0_f32);
    visuals.widgets.hovered.rounding = Rounding::same(5.0_f32);
    visuals.widgets.active.rounding = Rounding::same(5.0_f32);
    ctx.set_visuals(visuals);

    // Header Bar
    egui::TopBottomPanel::top("top_bar")
        .frame(
            Frame::none()
                .fill(bg_sub)
                .inner_margin(Margin::symmetric(16.0_f32, 10.0_f32))
                .stroke(Stroke::new(1.0_f32, border_subtle)),
        )
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("RESOLVE")
                        .color(accent_green)
                        .strong()
                        .size(13.0_f32),
                );
                ui.label(
                    RichText::new("SIDECAR")
                        .color(text_primary)
                        .strong()
                        .size(13.0_f32),
                );
                ui.add_space(4.0_f32);
                ui.label(
                    RichText::new("• v1.2")
                        .color(text_muted)
                        .size(11.0_f32),
                );
                ui.add_space(10.0_f32);

                let (pill_dot, pill_text, pill_stroke) = if app.resolve_running {
                    (accent_green, "Resolve: Connected", accent_green)
                } else {
                    (accent_amber, "Resolve: Offline", border_subtle)
                };

                Frame::none()
                    .fill(bg_card)
                    .rounding(Rounding::same(5.0_f32))
                    .stroke(Stroke::new(1.0_f32, pill_stroke))
                    .inner_margin(Margin::symmetric(8.0_f32, 3.0_f32))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (res, painter) = ui.allocate_painter(
                                Vec2::new(8.0_f32, 8.0_f32),
                                egui::Sense::hover(),
                            );
                            painter.circle_filled(res.rect.center(), 3.0_f32, pill_dot);
                            ui.label(
                                RichText::new(pill_text)
                                    .size(10.0_f32)
                                    .color(text_secondary),
                            );
                        });
                    });

                ui.add_space(6.0_f32);

                let (storage_dot, storage_text, storage_stroke) = match app.storage_state {
                    StorageMountState::MountedZ => {
                        (accent_green, "Storage: Z:\\ (Mounted)", accent_green)
                    }
                    StorageMountState::MountingZ => {
                        let pulse =
                            ((ctx.input(|i| i.time) * 3.0_f64).sin() * 0.5_f64 + 0.5_f64) as f32;
                        let alpha = (100.0_f32 + pulse * 155.0_f32) as u8;
                        let pulsing_amber = Color32::from_rgba_unmultiplied(
                            accent_amber.r(),
                            accent_amber.g(),
                            accent_amber.b(),
                            alpha,
                        );
                        (pulsing_amber, "Storage: Mounting Z:\\...", accent_amber)
                    }
                    StorageMountState::LocalProject => {
                        (accent_blue, "Storage: Local Project", border_subtle)
                    }
                };

                Frame::none()
                    .fill(bg_card)
                    .rounding(Rounding::same(5.0_f32))
                    .stroke(Stroke::new(1.0_f32, storage_stroke))
                    .inner_margin(Margin::symmetric(8.0_f32, 3.0_f32))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (res, painter) = ui.allocate_painter(
                                Vec2::new(8.0_f32, 8.0_f32),
                                egui::Sense::hover(),
                            );
                            painter.circle_filled(res.rect.center(), 3.0_f32, storage_dot);
                            ui.label(
                                RichText::new(storage_text)
                                    .size(10.0_f32)
                                    .color(text_secondary),
                            );
                        });
                    });

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button(RichText::new("Refresh").size(11.0_f32))
                        .clicked()
                    {
                        app.refresh_all();
                    }
                    ui.label(
                        RichText::new(format!("repo: {}", app.repo_path.display()))
                            .monospace()
                            .size(10.0_f32)
                            .color(text_muted),
                    );
                });
            });
        });

    // Bottom Status Bar
    egui::TopBottomPanel::bottom("bottom_bar")
        .frame(
            Frame::none()
                .fill(bg_sub)
                .inner_margin(Margin::symmetric(14.0_f32, 6.0_f32))
                .stroke(Stroke::new(1.0_f32, border_subtle)),
        )
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                let lower_status = app.status_message.to_lowercase();
                let dot_color = if lower_status.contains("error")
                    || lower_status.contains("not found")
                {
                    accent_red
                } else {
                    accent_green
                };
                let (res, painter) = ui.allocate_painter(
                    Vec2::new(10.0_f32, 10.0_f32),
                    egui::Sense::hover(),
                );
                painter.circle_filled(res.rect.center(), 3.0_f32, dot_color);
                ui.label(
                    RichText::new(&app.status_message)
                        .size(11.0_f32)
                        .color(text_secondary),
                );
            });
        });

    // Main Work Area
    egui::CentralPanel::default()
        .frame(Frame::none().fill(bg_base).inner_margin(Margin::same(14.0_f32)))
        .show(ctx, |ui| {
            ui.columns(2, |cols| {
                // LEFT COLUMN: Timeline History Tree
                Frame::none()
                    .fill(bg_sub)
                    .rounding(Rounding::same(5.0_f32))
                    .stroke(Stroke::new(1.0_f32, border_subtle))
                    .inner_margin(Margin::same(14.0_f32))
                    .show(&mut cols[0], |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("Timeline Commits")
                                    .strong()
                                    .size(13.0_f32)
                                    .color(text_primary),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        RichText::new(format!("{} nodes", app.commits.len()))
                                            .size(11.0_f32)
                                            .color(text_muted),
                                    );
                                },
                            );
                        });
                        ui.add_space(8.0_f32);
                        ui.separator();
                        ui.add_space(8.0_f32);

                        if app.commits.is_empty() {
                            ui.add_space(16.0_f32);
                            ui.vertical_centered(|ui| {
                                ui.label(
                                    RichText::new("No Timeline Commits Yet")
                                        .strong()
                                        .size(12.0_f32)
                                        .color(text_secondary),
                                );
                                ui.add_space(4.0_f32);
                                ui.label(
                                    RichText::new(
                                        "Capture the current Resolve project state as Node 0.",
                                    )
                                    .size(10.0_f32)
                                    .color(text_muted),
                                );
                                ui.add_space(10.0_f32);
                                let init_btn = egui::Button::new(
                                    RichText::new("Capture Root Baseline (Node 0)")
                                        .strong()
                                        .size(11.0_f32)
                                        .color(Color32::WHITE),
                                )
                                .fill(accent_green);
                                if ui.add(init_btn).clicked() {
                                    app.capture_timeline_baseline();
                                }
                            });
                        } else {
                            ScrollArea::vertical()
                                .id_source("commit_scroll")
                                .max_height(220.0_f32)
                                .show(ui, |ui| {
                                for (idx, c) in app.commits.iter().enumerate() {
                                    ui.horizontal(|ui| {
                                        let (response, painter) = ui.allocate_painter(
                                            Vec2::new(18.0_f32, 44.0_f32),
                                            egui::Sense::hover(),
                                        );
                                        let center = response.rect.center();

                                        if idx + 1 < app.commits.len() {
                                            painter.line_segment(
                                                [center, center + Vec2::new(0.0_f32, 30.0_f32)],
                                                Stroke::new(1.5_f32, border_subtle),
                                            );
                                        }
                                        painter.circle_filled(center, 4.0_f32, accent_green);

                                        ui.vertical(|ui| {
                                            ui.horizontal_wrapped(|ui| {
                                                ui.label(
                                                    RichText::new(&c.short_id)
                                                        .monospace()
                                                        .size(10.0_f32)
                                                        .color(accent_blue),
                                                );
                                                ui.label(
                                                    RichText::new(&c.author)
                                                        .size(11.0_f32)
                                                        .color(text_secondary),
                                                );
                                                ui.label(
                                                    RichText::new(&c.time)
                                                        .size(10.0_f32)
                                                        .color(text_muted),
                                                );
                                                if let Some(tag) = &c.tag {
                                                    ui.label(
                                                        RichText::new(format!(" {} ", tag))
                                                            .monospace()
                                                            .size(9.0_f32)
                                                            .background_color(Color32::from_rgb(
                                                                30, 41, 59,
                                                            ))
                                                            .color(accent_blue),
                                                    );
                                                }
                                            });
                                            ui.label(
                                                RichText::new(&c.message)
                                                    .size(12.0_f32)
                                                    .color(text_primary),
                                            );
                                            ui.add_space(4.0_f32);
                                        });
                                    });
                                }
                            });
                        }

                        ui.add_space(10.0_f32);
                        ui.separator();
                        ui.add_space(8.0_f32);

                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("Live Micro-History")
                                    .strong()
                                    .size(12.0_f32)
                                    .color(text_primary),
                            );
                            ui.label(
                                RichText::new("scratch/live-stream")
                                    .monospace()
                                    .size(10.0_f32)
                                    .color(accent_amber),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        RichText::new(format!("{} saves", app.micro_commits.len()))
                                            .size(10.0_f32)
                                            .color(text_muted),
                                    );
                                },
                            );
                        });
                        ui.add_space(6.0_f32);

                        if app.micro_commits.is_empty() {
                            ui.label(
                                RichText::new("Waiting for Resolve Live Save events (2s debounce)...")
                                    .size(10.0_f32)
                                    .color(text_muted),
                            );
                        } else {
                            let mut restore_oid: Option<String> = None;
                            ScrollArea::vertical()
                                .id_source("micro_scroll")
                                .max_height(160.0_f32)
                                .show(ui, |ui| {
                                    for mc in &app.micro_commits {
                                        Frame::none()
                                            .fill(bg_card)
                                            .rounding(Rounding::same(5.0_f32))
                                            .stroke(Stroke::new(1.0_f32, border_subtle))
                                            .inner_margin(Margin::symmetric(8.0_f32, 4.0_f32))
                                            .show(ui, |ui| {
                                                ui.horizontal(|ui| {
                                                    ui.label(
                                                        RichText::new(&mc.short_id)
                                                            .monospace()
                                                            .size(10.0_f32)
                                                            .color(accent_amber),
                                                    );
                                                    ui.label(
                                                        RichText::new(&mc.timestamp)
                                                            .monospace()
                                                            .size(10.0_f32)
                                                            .color(text_muted),
                                                    );
                                                    ui.label(
                                                        RichText::new(&mc.message)
                                                            .size(10.0_f32)
                                                            .color(text_secondary),
                                                    );
                                                    ui.with_layout(
                                                        egui::Layout::right_to_left(
                                                            egui::Align::Center,
                                                        ),
                                                        |ui| {
                                                            if ui
                                                                .button(
                                                                    RichText::new("Restore")
                                                                        .size(10.0_f32)
                                                                        .color(accent_blue),
                                                                )
                                                                .clicked()
                                                            {
                                                                restore_oid =
                                                                    Some(mc.full_id.clone());
                                                            }
                                                        },
                                                    );
                                                });
                                            });
                                        ui.add_space(4.0_f32);
                                    }
                                });
                            if let Some(oid) = restore_oid {
                                app.rollback_micro_save(&oid);
                            }
                        }
                    });

                // RIGHT COLUMN: Pull Requests & Diff Staging View
                Frame::none()
                    .fill(bg_sub)
                    .rounding(Rounding::same(5.0_f32))
                    .stroke(Stroke::new(1.0_f32, border_subtle))
                    .inner_margin(Margin::same(14.0_f32))
                    .show(&mut cols[1], |ui| {
                        ui.horizontal(|ui| {
                            if ui
                                .selectable_label(
                                    app.active_tab == 0,
                                    RichText::new("Incoming PRs").strong().size(12.0_f32),
                                )
                                .clicked()
                            {
                                app.active_tab = 0;
                            }
                            if ui
                                .selectable_label(
                                    app.active_tab == 1,
                                    RichText::new("+ Submit Edit Pass").strong().size(12.0_f32),
                                )
                                .clicked()
                            {
                                app.active_tab = 1;
                            }
                        });
                        ui.add_space(6.0_f32);
                        Frame::none()
                            .fill(bg_card)
                            .rounding(Rounding::same(5.0_f32))
                            .stroke(Stroke::new(1.0_f32, border_subtle))
                            .inner_margin(Margin::symmetric(10.0_f32, 6.0_f32))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new("experiment/")
                                            .monospace()
                                            .size(10.0_f32)
                                            .color(text_muted),
                                    );
                                    ui.add(
                                        egui::TextEdit::singleline(&mut app.sandbox_slug_input)
                                            .desired_width(105.0_f32)
                                            .margin(Margin::same(4.0_f32)),
                                    );
                                    if ui
                                        .button(
                                            RichText::new("AI Slug")
                                                .size(10.0_f32)
                                                .color(accent_blue),
                                        )
                                        .clicked()
                                    {
                                        app.on_click_suggest_sandbox_slug();
                                    }
                                    if ui
                                        .add(
                                            egui::Button::new(
                                                RichText::new("Branch / Sandbox")
                                                    .strong()
                                                    .size(10.0_f32)
                                                    .color(Color32::WHITE),
                                            )
                                            .fill(accent_amber),
                                        )
                                        .clicked()
                                    {
                                        app.on_click_sandbox();
                                    }
                                    if ui
                                        .add(
                                            egui::Button::new(
                                                RichText::new("Discard Sandbox")
                                                    .size(10.0_f32)
                                                    .color(accent_red),
                                            )
                                            .fill(bg_sub),
                                        )
                                        .clicked()
                                    {
                                        app.on_click_discard_sandbox();
                                    }
                                });
                            });
                        ui.add_space(8.0_f32);
                        ui.separator();
                        ui.add_space(10.0_f32);

                        if app.active_tab == 0 {
                            if app.pending_prs.is_empty() {
                                ui.add_space(20.0_f32);
                                ui.vertical_centered(|ui| {
                                    ui.label(
                                        RichText::new("No Pull Requests Pending")
                                            .color(text_muted)
                                            .size(12.0_f32),
                                    );
                                    ui.label(
                                        RichText::new("Timeline matches the master cut.")
                                            .color(text_muted)
                                            .size(10.0_f32),
                                    );
                                });
                            } else {
                                let mut accept_idx = None;
                                let mut review_action: Option<(String, &'static str)> = None;

                                    ScrollArea::vertical().id_source("pr_scroll").show(ui, |ui| {
                                        for (idx, pr) in app.pending_prs.iter().enumerate() {
                                            Frame::none()
                                                .fill(bg_card)
                                                .rounding(Rounding::same(5.0_f32))
                                                .stroke(Stroke::new(1.0_f32, border_subtle))
                                                .inner_margin(Margin::same(12.0_f32))
                                                .show(ui, |ui| {
                                                    ui.horizontal(|ui| {
                                                        ui.label(
                                                            RichText::new(&pr.title)
                                                                .strong()
                                                                .size(12.0_f32)
                                                                .color(text_primary),
                                                        );
                                                        ui.with_layout(
                                                            egui::Layout::right_to_left(
                                                                egui::Align::Center,
                                                            ),
                                                            |ui| {
                                                                ui.label(
                                                                    RichText::new(&pr.time_ago)
                                                                        .size(10.0_f32)
                                                                        .color(text_muted),
                                                                );
                                                                ui.label(
                                                                    RichText::new(format!(
                                                                        "by {}",
                                                                        pr.author
                                                                    ))
                                                                    .size(11.0_f32)
                                                                    .color(accent_blue),
                                                                );
                                                            },
                                                        );
                                                    });

                                                    ui.add_space(2.0_f32);
                                                    ui.label(
                                                        RichText::new(&pr.branch_name)
                                                            .monospace()
                                                            .size(10.0_f32)
                                                            .color(text_muted),
                                                    );

                                                    ui.add_space(4.0_f32);
                                                    ui.label(
                                                        RichText::new(&pr.notes)
                                                            .size(11.0_f32)
                                                            .color(text_secondary),
                                                    );

                                                    ui.add_space(8.0_f32);
                                                    ui.horizontal_wrapped(|ui| {
                                                        for trk in &pr.tracks {
                                                            ui.label(
                                                                RichText::new(format!(" {} ", trk))
                                                                    .size(10.0_f32)
                                                                    .background_color(
                                                                        Color32::from_rgb(
                                                                            18, 20, 26,
                                                                        ),
                                                                    )
                                                                    .color(accent_green),
                                                            );
                                                        }
                                                        ui.label(
                                                            RichText::new(format!(
                                                                "{} media files",
                                                                pr.media_count
                                                            ))
                                                            .size(10.0_f32)
                                                            .color(text_muted),
                                                        );
                                                    });

                                                    if !pr.reviews.is_empty() {
                                                        ui.add_space(8.0_f32);
                                                        Frame::none()
                                                            .fill(bg_sub)
                                                            .rounding(Rounding::same(5.0_f32))
                                                            .stroke(Stroke::new(
                                                                1.0_f32,
                                                                border_subtle,
                                                            ))
                                                            .inner_margin(Margin::same(8.0_f32))
                                                            .show(ui, |ui| {
                                                                for rev in &pr.reviews {
                                                                    let st_color = match rev
                                                                        .status
                                                                        .as_str()
                                                                    {
                                                                        "approved" => accent_green,
                                                                        "changes_requested" => {
                                                                            accent_amber
                                                                        }
                                                                        _ => accent_blue,
                                                                    };
                                                                    ui.horizontal_wrapped(|ui| {
                                                                        ui.label(
                                                                            RichText::new(format!(
                                                                                "[{}]",
                                                                                rev.status
                                                                            ))
                                                                            .monospace()
                                                                            .size(9.0_f32)
                                                                            .color(st_color),
                                                                        );
                                                                        ui.label(
                                                                            RichText::new(format!(
                                                                                "{}: {}",
                                                                                rev.author,
                                                                                rev.body
                                                                            ))
                                                                            .size(10.0_f32)
                                                                            .color(text_secondary),
                                                                        );
                                                                    });
                                                                }
                                                            });
                                                    }

                                                    ui.add_space(8.0_f32);
                                                    ui.horizontal(|ui| {
                                                        ui.add(
                                                            egui::TextEdit::singleline(
                                                                &mut app.review_draft_body,
                                                            )
                                                            .desired_width(170.0_f32)
                                                            .hint_text("Add peer review note...")
                                                            .margin(Margin::same(4.0_f32)),
                                                        );
                                                        if ui
                                                            .button(
                                                                RichText::new("Comment")
                                                                    .size(10.0_f32),
                                                            )
                                                            .clicked()
                                                        {
                                                            review_action = Some((
                                                                pr.branch_name.clone(),
                                                                "comment",
                                                            ));
                                                        }
                                                        if ui
                                                            .button(
                                                                RichText::new("Req Changes")
                                                                    .size(10.0_f32)
                                                                    .color(accent_amber),
                                                            )
                                                            .clicked()
                                                        {
                                                            review_action = Some((
                                                                pr.branch_name.clone(),
                                                                "changes_requested",
                                                            ));
                                                        }
                                                    });

                                                    ui.add_space(10.0_f32);
                                                    ui.horizontal(|ui| {
                                                        let btn = egui::Button::new(
                                                            RichText::new("Accept & Merge Cut")
                                                                .strong()
                                                                .size(11.0_f32)
                                                                .color(Color32::WHITE),
                                                        )
                                                        .fill(accent_green);

                                                        if ui.add(btn).clicked() {
                                                            accept_idx = Some(idx);
                                                        }
                                                    });
                                                });
                                            ui.add_space(8.0_f32);
                                        }
                                    });

                                    if let Some((branch, status)) = review_action {
                                        app.submit_review_comment(&branch, status);
                                    }
                                if let Some(idx) = accept_idx {
                                    app.accept_and_merge_pr(idx);
                                }
                            }
                        } else {
                            Frame::none()
                                .fill(bg_card)
                                .rounding(Rounding::same(5.0_f32))
                                .stroke(Stroke::new(1.0_f32, border_subtle))
                                .inner_margin(Margin::same(12.0_f32))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            RichText::new("Pass Title:")
                                                .size(11.0_f32)
                                                .color(text_secondary),
                                        );
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                if ui
                                                    .button(
                                                        RichText::new("AI Draft Title & Notes")
                                                            .size(10.0_f32)
                                                            .color(accent_blue),
                                                    )
                                                    .clicked()
                                                {
                                                    app.generate_ai_draft();
                                                }
                                            },
                                        );
                                    });
                                    ui.add_space(2.0_f32);
                                    ui.add(
                                        egui::TextEdit::singleline(&mut app.new_pr_title)
                                            .margin(Margin::same(6.0_f32))
                                            .text_color(text_primary),
                                    );
                                    ui.label(
                                        RichText::new(format!(
                                            "branch: pass/{}",
                                            crate::slugify_branch_title(&app.new_pr_title)
                                        ))
                                        .monospace()
                                        .size(10.0_f32)
                                        .color(text_muted),
                                    );

                                    ui.add_space(8.0_f32);
                                    ui.label(
                                        RichText::new("Changelog / Notes for Lead Editor:")
                                            .size(11.0_f32)
                                            .color(text_secondary),
                                    );
                                    ui.add_space(2.0_f32);
                                    ui.add(
                                        egui::TextEdit::multiline(&mut app.new_pr_notes)
                                            .desired_rows(4)
                                            .margin(Margin::same(6.0_f32))
                                            .text_color(text_primary),
                                    );

                                    ui.add_space(10.0_f32);
                                    ui.label(
                                        RichText::new("Edit Pass Scope Checklist:")
                                            .strong()
                                            .size(11.0_f32)
                                            .color(text_secondary),
                                    );
                                    ui.add_space(4.0_f32);

                                    Frame::none()
                                        .fill(bg_sub)
                                        .rounding(Rounding::same(5.0_f32))
                                        .stroke(Stroke::new(1.0_f32, border_subtle))
                                        .inner_margin(Margin::same(10.0_f32))
                                        .show(ui, |ui| {
                                            ui.horizontal_wrapped(|ui| {
                                                ui.checkbox(
                                                    &mut app.scope_checklist.video_cuts,
                                                    "Video Cuts (V1/V2)",
                                                );
                                                ui.checkbox(
                                                    &mut app.scope_checklist.audio_music,
                                                    "Audio / Music (A1-A3)",
                                                );
                                                ui.checkbox(
                                                    &mut app.scope_checklist.color_grade,
                                                    "Color Grade",
                                                );
                                                ui.checkbox(
                                                    &mut app.scope_checklist.titles_graphics,
                                                    "Titles / Graphics",
                                                );
                                                ui.checkbox(
                                                    &mut app.scope_checklist.sfx_foley,
                                                    "SFX / Foley",
                                                );
                                            });
                                        });

                                    ui.add_space(8.0_f32);
                                    if let Some(interchange) =
                                        crate::interchange::find_latest_interchange_file(
                                            &app.repo_path.join("Timelines"),
                                        )
                                    {
                                        let name = interchange
                                            .file_name()
                                            .and_then(|n| n.to_str())
                                            .unwrap_or("timeline");
                                        ui.label(
                                            RichText::new(format!(
                                                "Interchange ready: Timelines/{name}"
                                            ))
                                            .monospace()
                                            .size(10.0_f32)
                                            .color(accent_green),
                                        );
                                    } else {
                                        ui.label(
                                            RichText::new(
                                                "Place .drt / .otio / .otioz in Timelines/ (optional)",
                                            )
                                            .size(10.0_f32)
                                            .color(text_muted),
                                        );
                                    }

                                    ui.add_space(8.0_f32);
                                    ui.horizontal(|ui| {
                                        ui.add(
                                            egui::TextEdit::singleline(
                                                &mut app.screenshot_path_input,
                                            )
                                            .desired_width(210.0_f32)
                                            .hint_text("Frame grab / screenshot path...")
                                            .margin(Margin::same(5.0_f32)),
                                        );
                                        if ui
                                            .button(
                                                RichText::new("Attach Screenshot")
                                                    .size(10.0_f32),
                                            )
                                            .clicked()
                                        {
                                            app.attach_screenshot();
                                        }
                                    });
                                    if let Some(ref shot) = app.attached_screenshot_rel {
                                        ui.label(
                                            RichText::new(format!("Attached: {shot}"))
                                                .monospace()
                                                .size(10.0_f32)
                                                .color(accent_green),
                                        );
                                    }

                                    ui.add_space(12.0_f32);
                                    let submit_btn = egui::Button::new(
                                        RichText::new("Submit PR / Commit Pass")
                                            .strong()
                                            .size(11.0_f32)
                                            .color(Color32::WHITE),
                                    )
                                    .fill(accent_blue);

                                    if ui.add(submit_btn).clicked() {
                                        app.create_and_package_pr();
                                    }
                                });
                        }
                    });
            });
        });
}
