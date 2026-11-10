//! The Timeline ruler's own context menu (#1535): right-clicking the ruler, away from a marker,
//! opens the In/Out and marker menu at the playhead, not the clip menu of an earlier press.

use std::sync::mpsc::{Sender, channel};

use egui_kittest::Harness;
use filmcraft_engine::Session;
use filmcraft_ui_egui::FilmcraftApp;
use filmcraft_ui_egui::control::ControlRequest;
use serde_json::{Value, json};

struct Driver {
    harness: Harness<'static, FilmcraftApp>,
    tx: Sender<ControlRequest>,
}

impl Driver {
    fn demo() -> Self {
        let mut session = Session::default();
        session.execute("file.openDemoProject", json!({})).expect("demo project");
        let (tx, rx) = channel();
        let app = FilmcraftApp::new(session).with_control(rx);
        let harness = Harness::builder().with_size(egui::vec2(1600.0, 980.0)).with_max_steps(10_000).build_eframe(move |_cc| app);
        let mut d = Driver { harness, tx };
        d.frames(4);
        d
    }
    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            let ctx = self.harness.ctx.clone();
            let mut raw = std::mem::take(self.harness.input_mut());
            eframe::App::raw_input_hook(self.harness.state_mut(), &ctx, &mut raw);
            *self.harness.input_mut() = raw;
            self.harness.step();
        }
    }
    fn ok(&mut self, method: &str, params: Value) -> Value {
        let (req, reply) = ControlRequest::new(method, params.clone());
        self.tx.send(req).unwrap();
        for _ in 0..600 {
            self.frames(1);
            if let Ok(r) = reply.try_recv() {
                assert_eq!(r["ok"], true, "{method} {params}: {r}");
                self.frames(2);
                return r["result"].clone();
            }
        }
        panic!("no reply to {method} {params}");
    }
    /// Centre of an element, from `ui.elements`.
    fn centre(&mut self, id: &str) -> (f64, f64) {
        let v = self.ok("ui.elements", json!({"prefix": id}));
        let e = v.as_array().unwrap().iter().find(|e| e["id"] == id).unwrap_or_else(|| panic!("no element {id}")).clone();
        let r: Vec<f64> = e["rect"].as_array().unwrap().iter().map(|x| x.as_f64().unwrap()).collect();
        (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0)
    }
}

fn has(d: &mut Driver, id: &str) -> bool {
    let v = d.ok("ui.elements", json!({"prefix": id}));
    v.as_array().unwrap().iter().any(|e| e["id"] == id)
}

#[test]
fn right_clicking_the_ruler_opens_the_ruler_menu_not_a_stale_clip_menu() {
    let mut d = Driver::demo();
    // an earlier right-click on a clip leaves the clip menu's state behind
    let clip = d.harness.state().session.active_sequence().unwrap().video_tracks[0].items[0].id.0;
    d.ok("ui.click", json!({"id": format!("timeline.clip.{clip}"), "button": "right"}));
    assert!(has(&mut d, "timeline.clipMenu.edit.cut"), "a right-click on a clip opens the clip menu");
    d.ok("ui.key", json!({"key": "Escape"}));
    d.frames(2);
    // a right-click on the ruler, away from the markers, opens the ruler's menu
    let (x, y) = d.centre("timeline.ruler");
    d.ok("ui.click", json!({"x": x + 300.0, "y": y, "button": "right"}));
    assert!(has(&mut d, "timeline.rulerMenu.markers.markIn"), "Mark In is offered");
    assert!(has(&mut d, "timeline.rulerMenu.markers.markSelection"), "Mark Selection is offered");
    assert!(!has(&mut d, "timeline.clipMenu.edit.cut"), "and not the clip menu");
    // its entries act at the playhead
    d.ok("ui.click", json!({"id": "timeline.rulerMenu.markers.markIn"}));
    let playhead = d.harness.state().session.playhead();
    let q = d.harness.state().session.active_sequence().unwrap();
    assert_eq!(q.mark_in, Some(playhead), "Mark In set the In point at the playhead");
}

/// The work area bar sits in the ruler, and a right-click on it opens the same menu.
#[test]
fn right_clicking_the_work_area_bar_opens_the_ruler_menu() {
    let mut d = Driver::demo();
    d.ok("engine.execute", json!({"command": "sequence.setWorkArea", "params": {"start": 0, "end": 60 * 254_016_000_000_i64}}));
    d.frames(2);
    let (x, y) = d.centre("timeline.workArea");
    d.ok("ui.click", json!({"x": x, "y": y, "button": "right"}));
    assert!(has(&mut d, "timeline.rulerMenu.markers.markIn"), "the ruler's menu opens on the work area bar");
    assert!(!has(&mut d, "timeline.clipMenu.edit.cut"), "and not the clip menu");
}

/// Mark Selection in the ruler menu marks the span of the selected clips, whatever the playhead.
#[test]
fn mark_selection_in_the_ruler_menu_marks_the_selected_clips() {
    let mut d = Driver::demo();
    let (clip, start, end) = {
        let it = &d.harness.state().session.active_sequence().unwrap().video_tracks[0].items[0];
        (it.id.0, it.start, it.end())
    };
    d.ok("engine.execute", json!({"command": "timeline.select", "params": {"clips": [clip], "linked": false}}));
    d.ok("engine.execute", json!({"command": "playhead.set", "params": {"seconds": 7.0}}));
    d.frames(2);
    let (x, y) = d.centre("timeline.ruler");
    d.ok("ui.click", json!({"x": x + 300.0, "y": y, "button": "right"}));
    d.ok("ui.click", json!({"id": "timeline.rulerMenu.markers.markSelection"}));
    let frame = d.harness.state().session.sequence_rate().frame_duration();
    let q = d.harness.state().session.active_sequence().unwrap();
    assert_eq!(q.mark_in, Some(start), "the In point is the selection's start");
    assert_eq!(q.mark_out, Some(end - frame), "the Out point is its last frame");
}

fn ruler_label(d: &mut Driver) -> String {
    let v = d.ok("ui.elements", json!({"prefix": "timeline.ruler"}));
    v.as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == "timeline.ruler")
        .and_then(|e| e["label"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// Right-clicking the ruler offers Set Timeline to Tempo. OK stores the BPM and the ruler
/// announces it; Show timecode clears it. Cancel stores nothing.
#[test]
fn right_clicking_the_ruler_sets_the_timeline_to_a_tempo() {
    let mut d = Driver::demo();
    let (x, y) = d.centre("timeline.ruler");
    d.ok("ui.click", json!({"x": x + 300.0, "y": y, "button": "right"}));
    assert!(has(&mut d, "timeline.menu.setTempo"), "Set Timeline to Tempo is offered");
    d.ok("ui.click", json!({"id": "timeline.menu.setTempo"}));
    d.frames(3);
    assert!(has(&mut d, "timelineTempo.bpm"));
    d.ok("ui.click", json!({"id": "timelineTempo.cancel"}));
    d.frames(2);
    assert!(d.harness.state().session.active_sequence().unwrap().settings.tempo_bpm.is_none(), "Cancel stores nothing");

    d.ok("ui.click", json!({"x": x + 300.0, "y": y, "button": "right"}));
    d.ok("ui.click", json!({"id": "timeline.menu.setTempo"}));
    d.frames(3);
    d.ok("ui.click", json!({"id": "timelineTempo.bpm"}));
    d.frames(2);
    d.ok("ui.key", json!({"key": "Cmd+A"}));
    d.ok("ui.type", json!({"text": "96"}));
    d.ok("ui.key", json!({"key": "Enter"}));
    d.frames(2);
    d.ok("ui.click", json!({"id": "timelineTempo.ok"}));
    d.frames(3);
    assert_eq!(d.harness.state().session.active_sequence().unwrap().settings.tempo_bpm, Some(96.0));
    let label = ruler_label(&mut d);
    assert!(label.contains("96 BPM"), "{label}");

    d.ok("ui.click", json!({"x": x + 300.0, "y": y, "button": "right"}));
    d.ok("ui.click", json!({"id": "timeline.menu.setTempo"}));
    d.frames(3);
    d.ok("ui.click", json!({"id": "timelineTempo.clear"}));
    d.frames(3);
    assert_eq!(d.harness.state().session.active_sequence().unwrap().settings.tempo_bpm, None);
    assert!(!ruler_label(&mut d).contains("BPM"), "the ruler is back to timecode");
}

/// A right-click on the ruler offers Pearl Necklace, and Cancel closes it without laying clips.
#[test]
fn right_clicking_the_ruler_opens_pearl_necklace() {
    let mut d = Driver::demo();
    let (x, y) = d.centre("timeline.ruler");
    d.ok("ui.click", json!({"x": x + 300.0, "y": y, "button": "right"}));
    assert!(has(&mut d, "timeline.menu.pearlNecklace"), "Pearl Necklace is offered");
    d.ok("ui.click", json!({"id": "timeline.menu.pearlNecklace"}));
    d.frames(3);
    assert!(has(&mut d, "timelinePearl.folder"));
    assert!(has(&mut d, "timelinePearl.transition"));
    assert!(has(&mut d, "timelinePearl.cancel"));
    d.ok("ui.click", json!({"id": "timelinePearl.transition"}));
    d.frames(2);
    assert!(has(&mut d, "timelinePearl.transition.cross_dissolve"));
    d.ok("ui.click", json!({"id": "timelinePearl.transition.cross_dissolve"}));
    d.frames(2);
    assert!(has(&mut d, "timelinePearl.note"), "a transition offers a note length");
    d.ok("ui.click", json!({"id": "timelinePearl.cancel"}));
    d.frames(2);
    assert!(!has(&mut d, "timelinePearl.folder"), "Cancel closes the dialog");
}

/// A right-click on a clip, which is also the timeline, offers the same command.
#[test]
fn right_clicking_a_clip_offers_set_timeline_to_tempo() {
    let mut d = Driver::demo();
    let clip = d.harness.state().session.active_sequence().unwrap().video_tracks[0].items[0].id.0;
    d.ok("ui.click", json!({"id": format!("timeline.clip.{clip}"), "button": "right"}));
    assert!(has(&mut d, "timeline.menu.setTempo"), "the clip menu offers Set Timeline to Tempo");
}
