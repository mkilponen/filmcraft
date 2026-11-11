//! Clip Shuffler. The first press of the tool highlights video clips. A press while it is already
//! the current tool deals those shots into the same cuts at random and leaves the tool.
//!
//! Timeline presses do nothing else in this mode: no move, trim, razor, or slip.

use serde_json::{Value, json};

use crate::FilmcraftApp;
use crate::state::Tool;
use filmcraft_project::{ClipId, TrackKind};

/// A tools-panel or `tool.*` command picked `tool`. The Clip Shuffler toggles between highlighting
/// and shuffling.
pub fn on_tool_picked(app: &mut FilmcraftApp, tool: Tool) -> Result<Value, String> {
    if tool == Tool::ClipShuffler && app.ui.tool == Tool::ClipShuffler {
        return finish(app);
    }
    if app.ui.tool == Tool::ClipShuffler {
        app.ui.shuffle_clips.clear();
    }
    if tool == Tool::ClipShuffler {
        app.ui.shuffle_clips.clear();
    }
    app.ui.tool = tool;
    Ok(json!({"tool": format!("{tool:?}"), "shuffled": false}))
}

/// Add or remove a video clip from the highlight. Audio clips are left out.
pub fn toggle_clip(app: &mut FilmcraftApp, clip: ClipId) {
    let video =
        app.session.active_sequence().is_some_and(|q| q.find_item(clip).and_then(|(track, _)| q.track(track)).is_some_and(|t| t.kind == TrackKind::Video));
    if !video {
        return;
    }
    if let Some(i) = app.ui.shuffle_clips.iter().position(|id| *id == clip.0) {
        app.ui.shuffle_clips.remove(i);
    } else if app.ui.shuffle_clips.len() < 500 {
        app.ui.shuffle_clips.push(clip.0);
    }
}

fn finish(app: &mut FilmcraftApp) -> Result<Value, String> {
    if app.ui.shuffle_clips.len() < 2 {
        app.ui.status = tl!("Select at least two clips to shuffle.").to_string();
        return Ok(json!({"shuffled": false}));
    }
    let clips = app.ui.shuffle_clips.clone();
    match app.session.execute("timeline.shuffleClips", json!({"clips": clips})) {
        Ok(v) => {
            let n = v["clips"].as_array().map(Vec::len).unwrap_or(0);
            app.ui.shuffle_clips.clear();
            app.ui.tool = Tool::Selection;
            app.ui.status = tlf!("{n} clips shuffled", n = n);
            Ok(v)
        }
        Err(e) => {
            app.ui.status = e.to_string();
            Err(e.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_second_press_shuffles_the_highlighted_clips_and_leaves_the_tool() {
        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        app.session.execute("file.newSequence", json!({"name": "Shuffle", "fps": 24.0})).unwrap();
        let mut items = Vec::new();
        for name in ["A", "B", "C"] {
            items.push(app.session.execute("file.newColorMatte", json!({"name": name, "seconds": 5.0})).unwrap()["item"].as_u64().unwrap());
        }
        let mut clips = Vec::new();
        for (i, item) in items.iter().enumerate() {
            let placed = app
                .session
                .execute(
                    "timeline.place",
                    json!({"item": item, "frame": i * 24, "duration": {"frame": 24}, "sourceIn": {"frame": i * 12}, "video": true, "audio": false}),
                )
                .unwrap();
            clips.push(placed["clips"][0].as_u64().unwrap());
        }
        on_tool_picked(&mut app, Tool::ClipShuffler).unwrap();
        assert_eq!(app.ui.tool, Tool::ClipShuffler);
        on_tool_picked(&mut app, Tool::ClipShuffler).unwrap();
        assert_eq!(app.ui.tool, Tool::ClipShuffler, "nothing is highlighted yet, so the tool stays");

        for id in &clips {
            toggle_clip(&mut app, ClipId(*id));
        }
        assert_eq!(app.ui.shuffle_clips, clips);
        toggle_clip(&mut app, ClipId(clips[0]));
        toggle_clip(&mut app, ClipId(clips[0]));
        let before: Vec<_> =
            app.session.active_sequence().unwrap().video_tracks[0].items.iter().map(|it| (it.start.0, it.duration.0, it.item.0, it.source_in.0)).collect();
        on_tool_picked(&mut app, Tool::ClipShuffler).unwrap();
        assert_eq!(app.ui.tool, Tool::Selection);
        assert!(app.ui.shuffle_clips.is_empty());
        let after: Vec<_> =
            app.session.active_sequence().unwrap().video_tracks[0].items.iter().map(|it| (it.start.0, it.duration.0, it.item.0, it.source_in.0)).collect();
        assert_eq!(after.iter().map(|r| (r.0, r.1)).collect::<Vec<_>>(), before.iter().map(|r| (r.0, r.1)).collect::<Vec<_>>());
        let mut shots: Vec<_> = after.iter().map(|r| (r.2, r.3)).collect();
        let mut was: Vec<_> = before.iter().map(|r| (r.2, r.3)).collect();
        shots.sort();
        was.sort();
        assert_eq!(shots, was, "the shots are the same ones, in some order");
        if after.iter().zip(&before).any(|(a, b)| (a.2, a.3) != (b.2, b.3)) {
            assert_eq!(app.session.undo().as_deref(), Some("Shuffle Clips"));
        }
    }
}
