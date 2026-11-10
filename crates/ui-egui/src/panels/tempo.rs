//! Timeline right-click ▸ Set Timeline to Tempo…: a BPM for the ruler, which then counts beats
//! in 4/4 from the start of the sequence. OK runs `sequence.setTempo` (one undo step). Show
//! timecode clears it.
//!
//! Automation ids: `timelineTempo.bpm`, `timelineTempo.ok`, `timelineTempo.cancel`,
//! `timelineTempo.clear`.

use serde_json::json;

use crate::FilmcraftApp;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Draft {
    bpm: f64,
}

fn draft_id() -> egui::Id {
    egui::Id::unique("timeline-tempo-draft")
}

/// Open the dialog for the active sequence, starting from its tempo or 120 BPM.
pub fn open(app: &mut FilmcraftApp, ctx: &egui::Context) {
    if app.session.active_sequence().is_none() {
        return;
    }
    let bpm = app.session.active_sequence().and_then(|q| q.settings.tempo_bpm).unwrap_or(120.0);
    ctx.data_mut(|m| m.insert_temp(draft_id(), Some(Draft { bpm })));
}

pub fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let Some(Some(mut d)) = ctx.data(|m| m.get_temp::<Option<Draft>>(draft_id())) else { return };
    crate::widgets::revert_drag_on_escape(ctx, egui::Id::unique("timeline-tempo-before-drag"), &mut d);
    let mut close = false;
    let mut apply = false;
    let mut clear = false;
    let mut elems: Vec<(String, egui::Rect, String)> = Vec::new();
    let has_tempo = app.session.active_sequence().and_then(|q| q.settings.tempo_bpm).is_some();
    egui::Window::new(tl!("Set Timeline to Tempo"))
        .id(egui::Id::unique("Set Timeline to Tempo"))
        .collapsible(false)
        .resizable(false)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(crate::widgets::dialog_center(ctx))
        .show(ctx, |ui| {
            ui.set_min_width(340.0);
            ui.label(tl!("The ruler counts beats in 4/4 from the start of the timeline."));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(tl!("Tempo (BPM)"));
                let r = ui.add(crate::widgets::number_drag(&mut d.bpm).speed(1.0).range(1.0..=999.0).max_decimals(2));
                elems.push(("timelineTempo.bpm".into(), r.rect, format!("{}", d.bpm)));
            });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let c = ui.add_enabled(has_tempo, egui::Button::new(tl!("Show timecode")));
                elems.push(("timelineTempo.clear".into(), c.rect, "Show timecode".into()));
                if c.clicked() {
                    clear = true;
                }
                let cancel = ui.button(tl!("Cancel"));
                elems.push(("timelineTempo.cancel".into(), cancel.rect, "Cancel".into()));
                if cancel.clicked() {
                    close = true;
                }
                let ok = ui.add(egui::Button::new(egui::RichText::new(tl!("OK")).color(egui::Color32::WHITE)).fill(app.tokens.accent));
                elems.push(("timelineTempo.ok".into(), ok.rect, "OK".into()));
                if ok.clicked() {
                    apply = true;
                }
            });
        });
    for (id, r, l) in elems {
        app.auto.add(&id, r, &l);
    }
    if crate::widgets::escape_closes(ctx) {
        close = true;
    }
    if clear {
        match app.session.execute("sequence.setTempo", json!({"bpm": null})) {
            Ok(_) => app.ui.status = tl!("Timeline shows timecode").into(),
            Err(e) => app.ui.status = e.to_string(),
        }
        close = true;
    }
    if apply {
        match app.session.execute("sequence.setTempo", json!({"bpm": d.bpm})) {
            Ok(v) => {
                let bpm = v["bpm"].as_f64().unwrap_or(d.bpm);
                app.ui.status = tlf!("Timeline set to {bpm} BPM", bpm = format_bpm(bpm));
                close = true;
            }
            Err(e) => app.ui.status = e.to_string(),
        }
    }
    ctx.data_mut(|m| m.insert_temp(draft_id(), if close { None } else { Some(d) }));
}

fn format_bpm(bpm: f64) -> String {
    if bpm.fract() == 0.0 { format!("{bpm:.0}") } else { format!("{bpm:.2}") }
}
