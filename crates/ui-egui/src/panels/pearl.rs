//! Timeline right-click ▸ Pearl Necklace…: choose a folder, tick the videos, and lay each one
//! as a quarter note of the sequence tempo.
//!
//! Automation ids: `timelinePearl.folder`, `timelinePearl.ok`, `timelinePearl.cancel`,
//! `timelinePearl.all`, `timelinePearl.none`, `timelinePearl.file.<n>`,
//! `timelinePearl.transition`, `timelinePearl.transition.<id>`, `timelinePearl.note`,
//! `timelinePearl.note.<sixteenth|eighth|quarter>`, `timelinePearl.audio`,
//! `timelinePearl.audioClear`, `timelinePearl.sync`, `timelinePearl.density`,
//! `timelinePearl.energy`.

use serde_json::json;

use crate::FilmcraftApp;
use crate::native_dialogs::FileDialog;

#[derive(Clone, Debug, PartialEq)]
struct Row {
    path: String,
    name: String,
    on: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct Draft {
    folder: String,
    files: Vec<Row>,
    /// Effect id, or empty for hard cuts.
    transition: String,
    /// `sixteenth`, `eighth` or `quarter`.
    note: String,
    /// Path of the song under the row. Empty when none is attached.
    audio: String,
    /// Cut each video to the next hit in `audio`, at the timeline tempo.
    sync: bool,
    /// 0–100. 100 cuts every selected note. Lower values hold longer and end on a hit.
    density: f32,
    /// 0–100. Zero ignores loudness. 100 shortens the loud sections toward the selected note.
    energy: f32,
    error: Option<String>,
}

fn draft_id() -> egui::Id {
    egui::Id::unique("timeline-pearl-draft")
}

/// Open the dialog. Nothing is imported until OK.
pub fn open(ctx: &egui::Context) {
    ctx.data_mut(|m| {
        m.insert_temp(
            draft_id(),
            Some(Draft {
                folder: String::new(),
                files: Vec::new(),
                transition: String::new(),
                note: "quarter".into(),
                audio: String::new(),
                sync: false,
                density: 100.0,
                energy: 0.0,
                error: None,
            }),
        )
    });
}

pub fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let Some(Some(mut d)) = ctx.data(|m| m.get_temp::<Option<Draft>>(draft_id())) else { return };
    let mut close = false;
    let mut apply = false;
    let mut browse = false;
    let mut pick_audio = false;
    if d.audio.is_empty() {
        d.sync = false;
    }
    let mut all = false;
    let mut none = false;
    let mut elems: Vec<(String, egui::Rect, String)> = Vec::new();
    let tempo = app.session.active_sequence().and_then(|q| q.settings.tempo_bpm);
    let chosen = d.files.iter().filter(|f| f.on).count();
    egui::Window::new(tl!("Pearl Necklace"))
        .id(egui::Id::unique("Pearl Necklace"))
        .collapsible(false)
        .resizable(false)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(crate::widgets::dialog_center(ctx))
        .show(ctx, |ui| {
            ui.set_min_width(420.0);
            ui.label(tl!("Selected videos are laid in a row. With a song, they repeat until the song ends."));
            match tempo {
                Some(bpm) => {
                    ui.label(tlf!("At {bpm} BPM a quarter note is one beat.", bpm = format_bpm(bpm)));
                }
                None => {
                    ui.label(tl!("Set a timeline tempo first. A quarter note is one beat."));
                }
            };
            transition_row(ui, &mut d, &mut elems);
            audio_row(ui, &app.hooks, &mut d, &mut elems, &mut pick_audio);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let folder = if d.folder.is_empty() { tl!("No folder chosen").to_string() } else { d.folder.clone() };
                ui.label(folder);
                let can = FileDialog::Folder { at: Some(d.folder.clone()) }.available(&app.hooks);
                let r = ui.add_enabled(can, egui::Button::new(tl!("Choose Folder…")));
                elems.push(("timelinePearl.folder".into(), r.rect, "Choose Folder…".into()));
                if r.clicked() {
                    browse = true;
                }
            });
            ui.add_space(6.0);
            if !d.files.is_empty() {
                ui.horizontal(|ui| {
                    let a = ui.button(tl!("Select all"));
                    elems.push(("timelinePearl.all".into(), a.rect, "Select all".into()));
                    if a.clicked() {
                        all = true;
                    }
                    let n = ui.button(tl!("Select none"));
                    elems.push(("timelinePearl.none".into(), n.rect, "Select none".into()));
                    if n.clicked() {
                        none = true;
                    }
                });
                egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                    for (i, row) in d.files.iter_mut().enumerate() {
                        let r = ui.push_id(i, |ui| ui.checkbox(&mut row.on, &row.name)).inner;
                        elems.push((format!("timelinePearl.file.{i}"), r.rect, row.name.clone()));
                    }
                });
            }
            if let Some(err) = &d.error {
                ui.add_space(6.0);
                ui.label(egui::RichText::new(err).color(egui::Color32::from_rgb(0xc4, 0x50, 0x50)));
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let cancel = ui.button(tl!("Cancel"));
                elems.push(("timelinePearl.cancel".into(), cancel.rect, "Cancel".into()));
                if cancel.clicked() {
                    close = true;
                }
                let ready = tempo.is_some() && chosen > 0;
                let ok = ui.add_enabled(ready, egui::Button::new(egui::RichText::new(tl!("OK")).color(egui::Color32::WHITE)).fill(app.tokens.accent));
                elems.push(("timelinePearl.ok".into(), ok.rect, "OK".into()));
                if ok.clicked() {
                    apply = true;
                }
            });
        });
    for (id, r, l) in elems {
        app.auto.add(&id, r, &l);
    }
    if all {
        for row in &mut d.files {
            row.on = true;
        }
    }
    if none {
        for row in &mut d.files {
            row.on = false;
        }
    }
    if crate::widgets::escape_closes(ctx) {
        close = true;
    }
    if browse && !close {
        let folder = d.folder.clone();
        let ctx = ctx.clone();
        app.pick_ui(FileDialog::Folder { at: Some(folder) }, move |app, paths| {
            let Some(path) = paths.into_iter().next() else { return };
            let listed = app.session.execute("mediaBrowser.list", json!({"path": path, "fileTypes": "video"}));
            let Some(Some(mut draft)) = ctx.data(|m| m.get_temp::<Option<Draft>>(draft_id())) else { return };
            match listed {
                Ok(v) => {
                    draft.folder = path;
                    draft.files = rows_of(&v);
                    draft.error = if draft.files.is_empty() { Some(tl!("This folder has no video files.").to_string()) } else { None };
                }
                Err(e) => draft.error = Some(e.to_string()),
            }
            ctx.data_mut(|m| m.insert_temp(draft_id(), Some(draft)));
        });
    }
    if pick_audio && !close {
        let ctx = ctx.clone();
        app.pick_ui(FileDialog::open_file("Audio", filmcraft_media::AUDIO_EXTENSIONS), move |_app, paths| {
            let Some(path) = paths.into_iter().next() else { return };
            let Some(Some(mut draft)) = ctx.data(|m| m.get_temp::<Option<Draft>>(draft_id())) else { return };
            draft.audio = path;
            draft.error = None;
            ctx.data_mut(|m| m.insert_temp(draft_id(), Some(draft)));
        });
    }
    if apply && !close {
        let paths: Vec<String> = d.files.iter().filter(|f| f.on).map(|f| f.path.clone()).collect();
        let mut params = json!({"paths": paths});
        params["bin"] = json!(app.import_bin().0);
        if !d.transition.is_empty() || d.sync {
            params["transitionNote"] = json!(d.note);
        }
        if !d.transition.is_empty() {
            params["transition"] = json!(d.transition);
        }
        if !d.audio.is_empty() {
            params["audio"] = json!(d.audio);
            params["sync"] = json!(d.sync);
            if d.sync {
                params["density"] = json!(d.density as f64 / 100.0);
                params["energy"] = json!(d.energy as f64 / 100.0);
            }
        }
        match app.session.execute("timeline.pearlNecklace", params) {
            Ok(v) => {
                let n = v["clips"].as_array().map(Vec::len).unwrap_or(0);
                let transitions = v["transitions"].as_u64().unwrap_or(0);
                app.ui.status = if !d.audio.is_empty() && d.sync {
                    tlf!("{n} clips cut to the music", n = n)
                } else if !d.audio.is_empty() {
                    tlf!("{n} clips across the song", n = n)
                } else if transitions > 0 {
                    tlf!("{n} clips laid, one quarter note each, with {transitions} transitions", n = n, transitions = transitions)
                } else {
                    tlf!("{n} clips laid, one quarter note each", n = n)
                };
                close = true;
            }
            Err(e) => d.error = Some(e.to_string()),
        }
    }
    ctx.data_mut(|m| m.insert_temp(draft_id(), if close { None } else { Some(d) }));
}

const TRANSITION_IDS: &[&str] = &["", "cross_dissolve", "dip_to_black", "dip_to_white", "film_dissolve", "additive_dissolve", "wipe", "push", "slide"];

fn transition_row(ui: &mut egui::Ui, d: &mut Draft, elems: &mut Vec<(String, egui::Rect, String)>) {
    ui.add_space(8.0);
    ui.label(tl!("Transition between clips"));
    ui.horizontal(|ui| {
        let shown = transition_label(&d.transition);
        let combo = craft_ui::ComboBox::from_id_salt("pearl-transition").width(180.0).selected_text(&shown).show_ui(ui, |ui| {
            for id in TRANSITION_IDS {
                let label = transition_label(id);
                let r = ui.selectable_label(d.transition == *id, &label);
                if r.clicked() {
                    d.transition = (*id).to_string();
                }
                let auto = if id.is_empty() { "timelinePearl.transition.none".to_string() } else { format!("timelinePearl.transition.{id}") };
                elems.push((auto, r.rect, label));
            }
            ui.min_rect()
        });
        elems.push(("timelinePearl.transition".into(), combo.response.rect, shown));
        if !d.transition.is_empty() && !d.sync {
            ui.label(tl!("Length"));
            let shown = note_label(&d.note);
            let combo = craft_ui::ComboBox::from_id_salt("pearl-note").width(130.0).selected_text(&shown).show_ui(ui, |ui| {
                for id in ["sixteenth", "eighth", "quarter"] {
                    let label = note_label(id);
                    let r = ui.selectable_label(d.note == id, &label);
                    if r.clicked() {
                        d.note = id.to_string();
                    }
                    elems.push((format!("timelinePearl.note.{id}"), r.rect, label));
                }
                ui.min_rect()
            });
            elems.push(("timelinePearl.note".into(), combo.response.rect, shown));
        }
    });
}

fn audio_row(ui: &mut egui::Ui, hooks: &crate::hooks::HostHooks, d: &mut Draft, elems: &mut Vec<(String, egui::Rect, String)>, pick_audio: &mut bool) {
    ui.add_space(8.0);
    ui.label(tl!("Audio"));
    ui.horizontal(|ui| {
        let name = if d.audio.is_empty() {
            tl!("No audio attached").to_string()
        } else {
            std::path::Path::new(&d.audio).file_name().and_then(|n| n.to_str()).unwrap_or(&d.audio).to_string()
        };
        ui.label(name);
        let dialog = FileDialog::open_file("Audio", filmcraft_media::AUDIO_EXTENSIONS);
        let r = ui.add_enabled(dialog.available(hooks), egui::Button::new(tl!("Attach Audio…")));
        elems.push(("timelinePearl.audio".into(), r.rect, "Attach Audio…".into()));
        if r.clicked() {
            *pick_audio = true;
        }
        if !d.audio.is_empty() {
            let clear = ui.button(tl!("Remove Audio"));
            elems.push(("timelinePearl.audioClear".into(), clear.rect, "Remove Audio".into()));
            if clear.clicked() {
                d.audio.clear();
                d.sync = false;
            }
        }
    });
    let sync = ui.add_enabled(!d.audio.is_empty(), egui::Checkbox::new(&mut d.sync, tl!("Sync to music")));
    elems.push(("timelinePearl.sync".into(), sync.rect, "Sync to music".into()));
    if d.sync {
        ui.horizontal(|ui| {
            ui.label(tl!("Note"));
            let shown = note_label(&d.note);
            let combo = craft_ui::ComboBox::from_id_salt("pearl-sync-note").width(130.0).selected_text(&shown).show_ui(ui, |ui| {
                for id in ["sixteenth", "eighth", "quarter"] {
                    let label = note_label(id);
                    let r = ui.selectable_label(d.note == id, &label);
                    if r.clicked() {
                        d.note = id.to_string();
                    }
                    elems.push((format!("timelinePearl.note.{id}"), r.rect, label));
                }
                ui.min_rect()
            });
            elems.push(("timelinePearl.note".into(), combo.response.rect, shown));
        });
    }
    ui.label(tl!("Transient density"));
    let density = ui.add_enabled(d.sync, egui::Slider::new(&mut d.density, 0.0..=100.0).step_by(1.0));
    elems.push(("timelinePearl.density".into(), density.rect, format!("{:.0}", d.density)));
    ui.label(tl!("Full density cuts every selected note. Lower density holds across several notes and ends on the nearest hit."));
    ui.label(tl!("Track energy"));
    let energy = ui.add_enabled(d.sync, egui::Slider::new(&mut d.energy, 0.0..=100.0).step_by(1.0));
    elems.push(("timelinePearl.energy".into(), energy.rect, format!("{:.0}", d.energy)));
    ui.label(tl!(
        "Zero ignores loudness. A stretch is full on only when bass, mids, and treble are all present. Both sliders at full cut every selected note."
    ));
}

fn transition_label(id: &str) -> String {
    match id {
        "" => tl!("None"),
        "cross_dissolve" => tl!("Cross Dissolve"),
        "dip_to_black" => tl!("Dip to Black"),
        "dip_to_white" => tl!("Dip to White"),
        "film_dissolve" => tl!("Film Dissolve"),
        "additive_dissolve" => tl!("Additive Dissolve"),
        "wipe" => tl!("Wipe"),
        "push" => tl!("Push"),
        "slide" => tl!("Slide"),
        other => other,
    }
    .to_string()
}

fn note_label(id: &str) -> String {
    match id {
        "sixteenth" => tl!("16th note"),
        "eighth" => tl!("8th note"),
        "quarter" => tl!("Quarter note"),
        other => other,
    }
    .to_string()
}

fn rows_of(listed: &serde_json::Value) -> Vec<Row> {
    let mut rows: Vec<Row> = listed["entries"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|e| e["isDir"] != true && e["kind"] == "video")
                .filter_map(|e| {
                    let path = e["path"].as_str()?.to_string();
                    let name = e["name"].as_str().unwrap_or(&path).to_string();
                    Some(Row { path, name, on: true })
                })
                .take(500)
                .collect()
        })
        .unwrap_or_default();
    rows.sort_by_key(|a| a.name.to_lowercase());
    rows
}

fn format_bpm(bpm: f64) -> String {
    if bpm.fract() == 0.0 { format!("{bpm:.0}") } else { format!("{bpm:.2}") }
}
