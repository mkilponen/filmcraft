//! Timeline tool: shuffle the footage inside the selected video clips.
//!
//! Each chosen clip keeps its place and its length, so the cuts stay where they are. The shot
//! inside the cut — the media, where it starts, and how it plays — is dealt to a random one of
//! those slots. One undo step, labelled "Shuffle Clips".

use filmcraft_project::{ClipId, ItemId, Label, TimeInterpolation, TrackId, TrackItem, TrackKind};
use filmcraft_time::Tick;
use serde_json::{Value, json};

use crate::commands::{CommandSpec, bad, has_seq};
use crate::{Result, Session};

const CMD: &str = "timeline.shuffleClips";
const MAX_CLIPS: usize = 500;

pub fn commands() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: CMD,
        label: "Shuffle Clips",
        menu: &[],
        shortcut: None,
        params: r#"{"clips":[id],"seed":u64?}"#,
        enabled: has_seq,
        run: shuffle,
        journal: true,
        named: crate::commands::NamedKeys::NONE,
    }]
}

/// Where each slot reads from, for `n` clips and `seed`. Index `i` plays what was at `order[i]`.
/// The same seed and count always give the same deal.
pub fn order(n: usize, seed: u64) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..n).collect();
    let mut state = seed;
    for i in (1..n).rev() {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let span = u64::try_from(i).unwrap_or(u64::MAX).saturating_add(1);
        let j = usize::try_from(state % span).unwrap_or(0);
        idx.swap(i, j);
    }
    idx
}

fn shuffle(s: &mut Session, p: &Value) -> Result<Value> {
    has_seq(s).map_err(|e| bad(CMD, e))?;
    let ids = clip_ids(p)?;
    let seed = seed_param(p)?;
    let q = s.active_sequence().ok_or_else(|| bad(CMD, "no sequence"))?;
    let mut slots = Vec::with_capacity(ids.len());
    for id in &ids {
        let (track, it) = q.find_item(*id).ok_or_else(|| bad(CMD, format!("no clip {}", id.0)))?;
        let kind = q.track(track).map(|t| t.kind).unwrap_or(TrackKind::Video);
        if kind != TrackKind::Video {
            return Err(bad(CMD, "the clip shuffler shuffles video clips"));
        }
        slots.push(Slot { track, id: it.id, start: it.start, shot: Shot::from(it) });
    }
    slots.sort_by(|a, b| a.start.cmp(&b.start).then(a.track.cmp(&b.track)).then(a.id.cmp(&b.id)));
    let deal = order(slots.len(), seed);
    if deal.iter().enumerate().all(|(i, src)| *src == i) {
        let clips: Vec<u64> = slots.iter().map(|s| s.id.0).collect();
        return Ok(json!({"clips": clips, "changed": false, "seed": seed}));
    }
    let shots: Vec<Shot> = slots.iter().map(|s| s.shot.clone()).collect();
    let clips: Vec<u64> = slots.iter().map(|s| s.id.0).collect();
    s.edit_sequence("Shuffle Clips", |q, _, _| {
        for (i, slot) in slots.iter().enumerate() {
            let Some(src) = deal.get(i).copied() else { continue };
            let Some(shot) = shots.get(src) else { continue };
            let Some((_, it)) = q.find_item_mut(slot.id) else {
                return Err(bad(CMD, format!("no clip {}", slot.id.0)));
            };
            shot.apply(it);
        }
        Ok(())
    })?;
    Ok(json!({"clips": clips, "changed": true, "seed": seed}))
}

struct Slot {
    track: TrackId,
    id: ClipId,
    start: Tick,
    shot: Shot,
}

#[derive(Clone)]
struct Shot {
    item: ItemId,
    name: String,
    label: Label,
    custom_label_color: Option<filmcraft_project::HexColor>,
    source_in: Tick,
    speed: f64,
    reverse: bool,
    frame_hold: Option<Tick>,
    source_channels: Vec<u16>,
    maintain_pitch: bool,
    time_interpolation: TimeInterpolation,
    multicam: Option<filmcraft_project::MulticamSel>,
}

impl Shot {
    fn from(it: &TrackItem) -> Self {
        Shot {
            item: it.item,
            name: it.name.clone(),
            label: it.label,
            custom_label_color: it.custom_label_color,
            source_in: it.source_in,
            speed: it.speed,
            reverse: it.reverse,
            frame_hold: it.frame_hold,
            source_channels: it.source_channels.clone(),
            maintain_pitch: it.maintain_pitch,
            time_interpolation: it.time_interpolation,
            multicam: it.multicam,
        }
    }

    fn apply(&self, it: &mut TrackItem) {
        it.item = self.item;
        it.name = self.name.clone();
        it.label = self.label;
        it.custom_label_color = self.custom_label_color;
        it.source_in = self.source_in;
        it.speed = self.speed;
        it.reverse = self.reverse;
        it.frame_hold = self.frame_hold;
        it.source_channels = self.source_channels.clone();
        it.maintain_pitch = self.maintain_pitch;
        it.time_interpolation = self.time_interpolation;
        it.multicam = self.multicam;
    }
}

fn clip_ids(p: &Value) -> Result<Vec<ClipId>> {
    let arr = p.get("clips").and_then(Value::as_array).ok_or_else(|| bad(CMD, "`clips` must be a list of clip ids"))?;
    if arr.len() < 2 || arr.len() > MAX_CLIPS {
        return Err(bad(CMD, format!("shuffle needs 2 to {MAX_CLIPS} clips")));
    }
    let mut ids = Vec::with_capacity(arr.len());
    for v in arr {
        let id = v.as_u64().ok_or_else(|| bad(CMD, "`clips` must be a list of clip ids"))?;
        if ids.contains(&id) {
            return Err(bad(CMD, "a clip can be shuffled once"));
        }
        ids.push(id);
    }
    Ok(ids.into_iter().map(ClipId).collect())
}

fn seed_param(p: &Value) -> Result<u64> {
    match p.get("seed") {
        None => {
            let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(1);
            Ok(u64::try_from(nanos).unwrap_or(u64::MAX))
        }
        Some(Value::Number(n)) => n.as_u64().ok_or_else(|| bad(CMD, "`seed` must be a non-negative integer")),
        Some(_) => Err(bad(CMD, "`seed` must be a non-negative integer")),
    }
}
