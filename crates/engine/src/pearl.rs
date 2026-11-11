//! Timeline ▸ Pearl Necklace: import chosen videos and lay each one as a single quarter note.
//!
//! A quarter note is one beat of the sequence tempo, in 4/4. The clips sit in a row on the video
//! track, each starting on a beat, picture only. One undo step, labelled "Pearl Necklace".
//!
//! `timeline.pearlNecklace {"items":[id]}` lays project items. `{"paths":[str]}` imports those
//! video files first (optional `bin`) and then lays them, in the order given. `time` is the tick
//! the row is nearest to; omitted, it is the playhead. `transition` is a video transition id or
//! name (`none` omits it). `transitionNote` is `sixteenth`, `eighth` or `quarter` (default
//! `quarter`): the transition length, and the note the song is cut on. `audio` is one audio file.
//! The chosen videos repeat until that file ends. `sync` reads the song's hits at the sequence
//! tempo. `density` is 0 to 1: 1 cuts every selected note, and lower values hold a clip across
//! several notes and end it on the nearest hit. `energy` is 0 to 1. Zero does not read loudness.
//! Above zero, each section's loudness and spectrum shorten the full-band parts toward the
//! selected note, and density scales that hold. A stretch missing bass, mids, or treble is low
//! energy. Both at 1 cut every selected note.

use filmcraft_project::{ClipId, ItemId};
use filmcraft_time::{Tick, TimeRange};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, ItemPlacement, bad, has_seq, place_item};
use crate::{Result, Session};

const CMD: &str = "timeline.pearlNecklace";

/// Most source videos one necklace accepts. A larger list is refused, not cut.
const MAX_CLIPS: usize = 500;
/// Most clips the row may lay while it repeats across a song.
const MAX_PLACED: usize = 4_000;

pub fn commands() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: CMD,
        label: "Pearl Necklace…",
        menu: &[],
        shortcut: None,
        params: r#"{"items":[id]?,"paths":[str]?,"time":ticks?,"bin":binId?,"transition":str?,"transitionNote":"sixteenth|eighth|quarter"?,"audio":path?,"sync":bool=false,"density":0..1=1,"energy":0..1=0}"#,
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
    let audio_path = audio_path(p)?;
    let sync = crate::commands::bool_p(p, "sync").unwrap_or(false);
    if sync && audio_path.is_none() {
        return Err(bad(CMD, "attach an audio file to sync to the music"));
    }
    let density = if sync { density_param(p)? } else { 1.0 };
    let energy = if sync { energy_param(p)? } else { 0.0 };
    let note_steps = if audio_path.is_some() { Some(note_steps(p)?) } else { None };
    let adest = match &audio_path {
        Some(_) => Some(s.targeting().audio_dest.ok_or_else(|| bad(CMD, "no audio track"))?),
        None => None,
    };
    let (clips, transitions, audio_clip) = s.edit_group("Pearl Necklace", |s| {
        let ids = match requested {
            Request::Items(ids) => ids,
            Request::Paths(paths) => import_videos(s, p, &paths)?,
        };
        let audio_item = match &audio_path {
            Some(path) => Some(import_audio(s, p, path)?),
            None => None,
        };
        let cuts = match audio_item {
            Some(item) => {
                let steps = note_steps.ok_or_else(|| bad(CMD, "`transitionNote` is sixteenth, eighth or quarter"))?;
                Some(audio_cuts(s, item, bpm, density, energy, steps)?)
            }
            None => None,
        };
        let row = match &cuts {
            Some(bounds) => Tick(*bounds.last().ok_or_else(|| bad(CMD, "the audio file is empty"))?),
            None => Tick(
                beat.checked_mul(i64::try_from(ids.len()).map_err(|_| bad(CMD, "too many clips"))?)
                    .ok_or_else(|| bad(CMD, "the row runs past the end of the timeline"))?,
            ),
        };
        let placed_n = cuts.as_ref().map(|b| b.len().saturating_sub(1)).unwrap_or(ids.len());
        if placed_n == 0 || placed_n > MAX_PLACED {
            return Err(bad(
                CMD,
                format!("a necklace lays 1 to {MAX_PLACED} clips; this song would need {placed_n}. Lower the transient density or the track energy"),
            ));
        }
        let mut out = Vec::with_capacity(placed_n);
        for i in 0..placed_n {
            let item = ids[i % ids.len()];
            let pi = s.project.item(item).ok_or_else(|| bad(CMD, "no such item"))?;
            if !pi.has_video() {
                return Err(bad(CMD, "each clip needs a picture"));
            }
            let (at, gap) = match &cuts {
                Some(bounds) => {
                    let at = start.0.checked_add(bounds[i]).ok_or_else(|| bad(CMD, "the row runs past the end of the timeline"))?;
                    (at, bounds[i + 1] - bounds[i])
                }
                None => {
                    let at = start
                        .0
                        .checked_add(beat.checked_mul(i as i64).ok_or_else(|| bad(CMD, "too many clips"))?)
                        .ok_or_else(|| bad(CMD, "the row runs past the end of the timeline"))?;
                    (at, beat)
                }
            };
            let dur = clip_duration(pi.duration(), Tick(gap), frame);
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
        let audio_clip = match (audio_item, adest) {
            (Some(item), Some(dest)) => {
                let media = s.project.item(item).map(|i| i.duration()).unwrap_or(Tick::ZERO);
                let dur = if media.0 > 0 { media.max(frame) } else { row.max(frame) };
                let placed = place_item(
                    s,
                    ItemPlacement {
                        item,
                        range: TimeRange::new(Tick::ZERO, dur),
                        at: start,
                        vdest: None,
                        adest: Some(dest),
                        insert: false,
                        label: "Pearl Necklace",
                        audio_split: None,
                    },
                )?;
                placed.first().map(|c| c.0)
            }
            _ => None,
        };
        let transitions = match &transition {
            Some((effect, frames)) => lay_transitions(s, &out, effect, *frames)?,
            None => 0,
        };
        Ok((out.iter().map(|c| c.0).collect::<Vec<_>>(), transitions, audio_clip))
    })?;
    Ok(
        json!({"clips": clips, "start": start.0, "beat": beat, "transitions": transitions, "audio": audio_clip, "sync": sync, "density": density, "energy": energy}),
    )
}

fn density_param(p: &Value) -> Result<f64> {
    match crate::commands::f64_p(p, "density") {
        None => Ok(1.0),
        Some(d) if d.is_finite() && (0.0..=1.0).contains(&d) => Ok(d),
        Some(_) => Err(bad(CMD, "transient density is from 0 to 1")),
    }
}

fn energy_param(p: &Value) -> Result<f64> {
    match crate::commands::f64_p(p, "energy") {
        None => Ok(0.0),
        Some(e) if e.is_finite() && (0.0..=1.0).contains(&e) => Ok(e),
        Some(_) => Err(bad(CMD, "track energy is from 0 to 1")),
    }
}

fn note_steps(p: &Value) -> Result<i64> {
    match crate::commands::str_p(p, "transitionNote").unwrap_or("quarter") {
        "quarter" => Ok(4),
        "eighth" => Ok(2),
        "sixteenth" => Ok(1),
        other => Err(bad(CMD, format!("`transitionNote` is sixteenth, eighth or quarter, not `{other}`"))),
    }
}

/// Cut ticks across the whole audio file. The videos repeat over these gaps.
fn audio_cuts(s: &Session, item: ItemId, bpm: f64, density: f64, energy: f64, note_steps: i64) -> Result<Vec<i64>> {
    let sr = s.active_sequence().map(|q| q.settings.sample_rate).unwrap_or(48_000).max(1);
    let media = s.project.item(item).map(|i| i.duration()).unwrap_or(Tick::ZERO);
    if media.0 <= 0 {
        return Err(bad(CMD, "the audio file is empty"));
    }
    let len = media.to_units_floor(i64::from(sr));
    if len <= 0 {
        return Err(bad(CMD, "the audio file is empty"));
    }
    let step = filmcraft_audio_dsp::rhythm::sixteenth_samples(sr, bpm).map_err(|e| bad(CMD, e.to_string()))?;
    // Full density is already one note in every section, so loudness cannot shorten it.
    // Zero energy does not read loudness at all.
    let (hits, sections) = if density < 1.0 {
        let channels = load_audio(s, item, sr, len)?;
        let refs: Vec<&[f32]> = channels.iter().map(Vec::as_slice).collect();
        let hits = filmcraft_audio_dsp::rhythm::map_transients(&refs, sr, bpm)
            .map_err(|e| bad(CMD, e.to_string()))?
            .transients
            .into_iter()
            .map(|t| t.sample)
            .collect();
        let sections = if energy > 0.0 { filmcraft_audio_dsp::rhythm::map_sections(&refs, sr, bpm).map_err(|e| bad(CMD, e.to_string()))? } else { Vec::new() };
        (hits, sections)
    } else {
        (Vec::new(), Vec::new())
    };
    let samples = filmcraft_audio_dsp::rhythm::plan_cuts_with_energy(&hits, step, density, energy, &sections, note_steps, len);
    let mut bounds = Vec::with_capacity(samples.len());
    for sample in samples {
        let tick = if sample >= len { media.0 } else { Tick::from_units(sample, i64::from(sr)).0 };
        if bounds.last().copied().is_none_or(|last| tick > last) {
            bounds.push(tick);
        }
    }
    if bounds.last().copied().is_none_or(|last| last < media.0) {
        bounds.push(media.0);
    }
    Ok(bounds)
}

fn audio_path(p: &Value) -> Result<Option<String>> {
    match crate::commands::str_p(p, "audio") {
        Some(path) if !path.is_empty() => {
            if !is_audio_file(path) {
                return Err(bad(CMD, "audio must be an audio file"));
            }
            Ok(Some(path.to_string()))
        }
        _ => Ok(None),
    }
}

fn is_audio_file(path: &str) -> bool {
    let ext = std::path::Path::new(path).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    filmcraft_media::AUDIO_EXTENSIONS.contains(&ext.as_str())
}

fn import_audio(s: &mut Session, p: &Value, path: &str) -> Result<ItemId> {
    let mut q = json!({"paths": [path], "imageSequence": false});
    if let Some(bin) = p.get("bin") {
        q["bin"] = bin.clone();
    }
    let imported = s.execute("file.import", q)?;
    if let Some(msg) = imported["errors"].as_array().and_then(|a| a.first()).and_then(Value::as_str) {
        return Err(bad(CMD, msg));
    }
    let id = imported["items"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(Value::as_u64)
        .map(ItemId)
        .ok_or_else(|| bad(CMD, "the audio file could not be imported"))?;
    let item = s.project.item(id).ok_or_else(|| bad(CMD, "the audio file could not be imported"))?;
    if !item.has_audio() {
        return Err(bad(CMD, "the audio file has no audio"));
    }
    Ok(id)
}

/// The file's samples at `sr`, capped at ten minutes. `len` is the caller's media length in samples.
fn load_audio(s: &Session, item: ItemId, sr: u32, len: i64) -> Result<Vec<Vec<f32>>> {
    let max = i64::try_from(u64::from(sr).saturating_mul(filmcraft_audio_dsp::remix::MAX_ANALYSIS_SECONDS)).unwrap_or(i64::MAX);
    let len = len.min(max);
    if len <= 0 {
        return Err(bad(CMD, "the audio file is empty"));
    }
    let frames = usize::try_from(len).map_err(|_| bad(CMD, "the audio is longer than 10 minutes"))?;
    let provider = s.media.provider(s.project.clone(), s.services.clone());
    let src = filmcraft_render::SourceProvider::source(&provider, item).ok_or_else(|| bad(CMD, "the audio file could not be read"))?;
    let buf = src.audio(0, frames, sr).map_err(|e| bad(CMD, format!("reading the audio: {e}")))?;
    Ok(buf.channels)
}

/// A video transition to lay on each cut, and its length in frames. Absent when the row is hard cuts.
fn transition_request(p: &Value, beat: i64, rate: filmcraft_time::FrameRate) -> Result<Option<(String, i64)>> {
    let Some(name) = crate::commands::str_p(p, "transition") else { return Ok(None) };
    if name.is_empty() || name.eq_ignore_ascii_case("none") {
        return Ok(None);
    }
    let def = filmcraft_project::vtransition::find_transition(name, filmcraft_project::EffectKind::VideoTransition)
        .ok_or_else(|| bad(CMD, format!("unknown video transition `{name}`")))?;
    let note = crate::commands::str_p(p, "transitionNote").unwrap_or("quarter");
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
