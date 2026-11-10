//! Timeline ▸ Pearl Necklace: import chosen videos and lay each one as a single quarter note.
//!
//! A quarter note is one beat of the sequence tempo, in 4/4. The clips sit in a row on the video
//! track, each starting on a beat, picture only. One undo step, labelled "Pearl Necklace".
//!
//! `timeline.pearlNecklace {"items":[id]}` lays project items. `{"paths":[str]}` imports those
//! video files first (optional `bin`) and then lays them, in the order given. `time` is the tick
//! the row is nearest to; omitted, it is the playhead. `transition` is a video transition id or
//! name (`none` omits it). `transitionNote` is `sixteenth`, `eighth` or `quarter` (default
//! `eighth`): how long that transition is, centered on each cut where two clips meet.

use filmcraft_project::{ClipId, ItemId};
use filmcraft_time::{Tick, TimeRange};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, ItemPlacement, bad, has_seq, place_item};
use crate::{Result, Session};

const CMD: &str = "timeline.pearlNecklace";

/// Most videos one necklace takes. A larger list is refused, not cut.
const MAX_CLIPS: usize = 500;

pub fn commands() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: CMD,
        label: "Pearl Necklace…",
        menu: &[],
        shortcut: None,
        params: r#"{"items":[id]?,"paths":[str]?,"time":ticks?,"bin":binId?,"transition":str?,"transitionNote":"sixteenth|eighth|quarter"?}"#,
        enabled: has_seq,
        run: lay,
        journal: true,
        named: crate::commands::NamedKeys::NONE,
    }]
}

fn lay(s: &mut Session, p: &Value) -> Result<Value> {
    has_seq(s).map_err(|e| bad(CMD, e))?;
    let bpm = s.active_sequence().and_then(|q| q.settings.tempo_bpm).ok_or_else(|| bad(CMD, "set a timeline tempo first"))?;
    let beat = filmcraft_time::ticks_per_beat(bpm).ok_or_else(|| bad(CMD, "set a timeline tempo first"))?;
    let origin = match p.get("time") {
        None => s.playhead(),
        Some(_) => tick_param(p)?,
    };
    let vdest = s.targeting().video_dest.ok_or_else(|| bad(CMD, "no video track"))?;
    let requested = requested(p)?;
    if let Request::Items(ids) = &requested {
        for id in ids {
            let pi = s.project.item(*id).ok_or_else(|| bad(CMD, "no such item"))?;
            if !pi.has_video() {
                return Err(bad(CMD, "each clip needs a picture"));
            }
        }
    }
    let start = nearest_beat(origin, beat);
    let rate = s.sequence_rate();
    let frame = rate.frame_duration();
    let transition = transition_request(p, beat, rate)?;
    let (clips, transitions) = s.edit_group("Pearl Necklace", |s| {
        let ids = match requested {
            Request::Items(ids) => ids,
            Request::Paths(paths) => import_videos(s, p, &paths)?,
        };
        let mut out = Vec::with_capacity(ids.len());
        for (i, item) in ids.iter().copied().enumerate() {
            let pi = s.project.item(item).ok_or_else(|| bad(CMD, "no such item"))?;
            if !pi.has_video() {
                return Err(bad(CMD, "each clip needs a picture"));
            }
            let dur = clip_duration(pi.duration(), Tick(beat), frame);
            let at = start
                .0
                .checked_add(beat.checked_mul(i as i64).ok_or_else(|| bad(CMD, "too many clips"))?)
                .ok_or_else(|| bad(CMD, "the row runs past the end of the timeline"))?;
            let placed = place_item(
                s,
                ItemPlacement {
                    item,
                    range: TimeRange::new(Tick::ZERO, dur),
                    at: Tick(at),
                    vdest: Some(vdest),
                    adest: None,
                    insert: false,
                    label: "Pearl Necklace",
                    audio_split: None,
                },
            )?;
            out.extend(placed.iter().copied());
        }
        let transitions = match &transition {
            Some((effect, frames)) => lay_transitions(s, &out, effect, *frames)?,
            None => 0,
        };
        Ok((out.iter().map(|c| c.0).collect::<Vec<_>>(), transitions))
    })?;
    Ok(json!({"clips": clips, "start": start.0, "beat": beat, "transitions": transitions}))
}

/// A video transition to lay on each cut, and its length in frames. Absent when the row is hard cuts.
fn transition_request(p: &Value, beat: i64, rate: filmcraft_time::FrameRate) -> Result<Option<(String, i64)>> {
    let Some(name) = crate::commands::str_p(p, "transition") else { return Ok(None) };
    if name.is_empty() || name.eq_ignore_ascii_case("none") {
        return Ok(None);
    }
    let def = filmcraft_project::vtransition::find_transition(name, filmcraft_project::EffectKind::VideoTransition)
        .ok_or_else(|| bad(CMD, format!("unknown video transition `{name}`")))?;
    let note = crate::commands::str_p(p, "transitionNote").unwrap_or("eighth");
    Ok(Some((def.id.to_string(), note_frames(note, beat, rate)?)))
}

/// `note` as a fraction of one beat, in whole frames, and at least one frame.
fn note_frames(note: &str, beat: i64, rate: filmcraft_time::FrameRate) -> Result<i64> {
    let parts = match note {
        "sixteenth" => 4,
        "eighth" => 2,
        "quarter" => 1,
        _ => return Err(bad(CMD, "`transitionNote` is sixteenth, eighth or quarter")),
    };
    let beat_frames = rate.frame_at(Tick(beat));
    if beat_frames < 1 {
        return Err(bad(CMD, "the beat is shorter than one frame"));
    }
    Ok((beat_frames / parts).max(1))
}

/// `effect` on each cut where two necklace clips meet. A short clip leaves a gap, and that cut
/// is skipped. One failure rolls the whole necklace back.
fn lay_transitions(s: &mut Session, clips: &[ClipId], effect: &str, frames: i64) -> Result<u64> {
    let incoming: Vec<ClipId> = clips.windows(2).filter(|pair| abut(s, pair[0], pair[1])).map(|pair| pair[1]).collect();
    let mut n = 0u64;
    for clip in incoming {
        s.execute("sequence.applyVideoTransition", json!({"clip": clip.0, "edge": "in", "effect": effect, "frames": frames, "align": "center"}))?;
        n += 1;
    }
    Ok(n)
}

fn abut(s: &Session, left: ClipId, right: ClipId) -> bool {
    let Some(q) = s.active_sequence() else { return false };
    let span = |id: ClipId| q.video_tracks.iter().find_map(|tr| tr.items.iter().find(|it| it.id == id).map(|it| (it.start, it.end())));
    match (span(left), span(right)) {
        (Some((_, end)), Some((start, _))) => end == start,
        _ => false,
    }
}

enum Request {
    Items(Vec<ItemId>),
    Paths(Vec<String>),
}

fn requested(p: &Value) -> Result<Request> {
    match (p.get("items"), p.get("paths")) {
        (Some(_), Some(_)) => Err(bad(CMD, "give `items` or `paths`, not both")),
        (None, None) => Err(bad(CMD, "give `items` or `paths`")),
        (Some(v), None) => Ok(Request::Items(item_ids(v)?)),
        (None, Some(v)) => Ok(Request::Paths(video_paths(v)?)),
    }
}

fn item_ids(v: &Value) -> Result<Vec<ItemId>> {
    let ids = v.as_array().ok_or_else(|| bad(CMD, "`items` must be a list of item ids"))?;
    if ids.is_empty() || ids.len() > MAX_CLIPS {
        return Err(bad(CMD, format!("`items` needs 1 to {MAX_CLIPS} item ids")));
    }
    ids.iter().map(|v| v.as_u64().map(ItemId).ok_or_else(|| bad(CMD, "`items` must be a list of item ids"))).collect()
}

fn video_paths(v: &Value) -> Result<Vec<String>> {
    let paths = v.as_array().ok_or_else(|| bad(CMD, "`paths` must be a list of video files"))?;
    if paths.is_empty() || paths.len() > MAX_CLIPS {
        return Err(bad(CMD, format!("`paths` needs 1 to {MAX_CLIPS} video files")));
    }
    paths
        .iter()
        .map(|v| {
            let path = v.as_str().ok_or_else(|| bad(CMD, "`paths` must be a list of video files"))?;
            if !is_video_file(path) {
                return Err(bad(CMD, "paths must be video files"));
            }
            Ok(path.to_string())
        })
        .collect()
}

fn is_video_file(path: &str) -> bool {
    let ext = std::path::Path::new(path).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    filmcraft_media::VIDEO_EXTENSIONS.contains(&ext.as_str())
}

fn import_videos(s: &mut Session, p: &Value, paths: &[String]) -> Result<Vec<ItemId>> {
    let mut q = json!({"paths": paths, "imageSequence": false});
    if let Some(bin) = p.get("bin") {
        q["bin"] = bin.clone();
    }
    let imported = s.execute("file.import", q)?;
    if let Some(msg) = imported["errors"].as_array().and_then(|a| a.first()).and_then(Value::as_str) {
        return Err(bad(CMD, msg));
    }
    let ids: Vec<ItemId> = imported["items"].as_array().map(|a| a.iter().filter_map(|v| v.as_u64().map(ItemId)).collect()).unwrap_or_default();
    if ids.len() != paths.len() {
        return Err(bad(CMD, "a video could not be imported"));
    }
    Ok(ids)
}

fn tick_param(p: &Value) -> Result<Tick> {
    match p.get("time") {
        Some(Value::Number(n)) => {
            let t = n.as_i64().ok_or_else(|| bad(CMD, "`time` must be a tick count"))?;
            if t < 0 { Err(bad(CMD, "`time` must not be negative")) } else { Ok(Tick(t)) }
        }
        _ => Err(bad(CMD, "`time` must be a tick count")),
    }
}

/// Nearest beat at `step` ticks. A tie stays on the earlier beat. `step` is a positive beat length.
fn nearest_beat(tick: Tick, step: i64) -> Tick {
    let base = tick.0.div_euclid(step).saturating_mul(step);
    let next = base.saturating_add(step);
    let earlier = tick.0.saturating_sub(base) <= next.saturating_sub(tick.0);
    Tick(if earlier { base } else { next }).max(Tick::ZERO)
}

/// How long one bead is: one beat, or the whole video when that is shorter, and at least one frame.
fn clip_duration(available: Tick, beat: Tick, frame: Tick) -> Tick {
    let want = if available.0 > 0 { available.min(beat) } else { beat };
    want.max(frame)
}
