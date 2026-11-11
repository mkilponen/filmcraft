//! A map of where a song hits, at a tempo the caller already knows.
//!
//! The onset envelope is the same spectral flux Clip ▸ Remix uses. Each peak is snapped to the
//! nearest 16th note of `bpm`, counted from the first sample. A tie stays on the earlier 16th.
//! A 16th is kept when it is louder than its neighbours and at least half as strong as the
//! loudest hit, so a quiet hat under a kick does not become its own cut.

use crate::biquad::{Biquad, Coeffs, FilterType};
use crate::loudness::{energy_to_lufs, k_prefilter, k_rlb};
use crate::remix::{MAX_ANALYSIS_SAMPLE_RATE, MAX_ANALYSIS_SECONDS, onset_envelope};

/// Windows this close in loudness, in LU, stay one section. A jump at least this large starts
/// another, so a louder part of the song can be cut faster than a quieter one.
const SECTION_MERGE_LU: f64 = 3.0;
/// BS.1770 absolute gate. Quieter than this, a window is silence and has no intensity.
const SILENCE_LUFS: f64 = -70.0;
/// Lowpass edge of the bass band, and the highpass edge of the mids. Hz.
const BASS_HZ: f64 = 200.0;
/// Highpass edge of the treble band, and the lowpass edge of the mids. Hz.
const TREBLE_HZ: f64 = 4_000.0;
/// After each band is leveled per octave, a gap no wider than this still counts as the whole spectrum.
const BAND_GAP_FULL_DB: f64 = 6.0;
/// A wider gap than this means bass, mids, or treble is missing, so the stretch is low energy.
const BAND_GAP_THIN_DB: f64 = 12.0;

/// One rhythmic hit, on a 16th-note of the given tempo.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transient {
    /// Samples from the start of the analysed audio.
    pub sample: i64,
    /// Onset strength of that 16th, after the envelope is normalised.
    pub strength: f32,
}

/// One stretch of the song at a similar loudness.
///
/// `intensity` is 1 when the stretch is loud and bass, mids, and treble are all present.
/// It is 0 for silence, for a song with no contrast, and for a loud stretch that is missing a band.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Section {
    /// First sample of the stretch.
    pub start: i64,
    /// Sample just past the stretch.
    pub end: i64,
    /// K-weighted loudness of the stretch, in LUFS. −∞ when it is silence.
    pub lufs: f64,
    /// 0 to 1, compared with the rest of this song.
    pub intensity: f64,
}

/// The song's hits on the tempo grid.
#[derive(Clone, Debug, PartialEq)]
pub struct TransientMap {
    pub sample_rate: u32,
    pub bpm: f64,
    /// One 16th note, in samples. At least 1.
    pub step: i64,
    pub transients: Vec<Transient>,
}

/// Why a recording could not be mapped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RhythmError {
    InvalidSampleRate,
    InvalidTempo,
    TooLong,
    Empty,
}

impl std::fmt::Display for RhythmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidSampleRate => "the audio sample rate is not supported",
            Self::InvalidTempo => "the tempo must be from 1 to 999 BPM",
            Self::TooLong => "the audio is longer than 10 minutes",
            Self::Empty => "the audio file is empty",
        })
    }
}

/// Read `channels` (summed to mono) as hits at `bpm`. An empty `transients` list means the
/// recording had no hit strong enough to keep. The same audio and tempo always give the same map.
pub fn map_transients(channels: &[&[f32]], sample_rate: u32, bpm: f64) -> Result<TransientMap, RhythmError> {
    if sample_rate == 0 || sample_rate > MAX_ANALYSIS_SAMPLE_RATE {
        return Err(RhythmError::InvalidSampleRate);
    }
    if !bpm.is_finite() || !(1.0..=999.0).contains(&bpm) {
        return Err(RhythmError::InvalidTempo);
    }
    let len = channels.iter().map(|c| c.len()).max().unwrap_or(0);
    if len == 0 {
        return Err(RhythmError::Empty);
    }
    if (len as u64) > u64::from(sample_rate).saturating_mul(MAX_ANALYSIS_SECONDS) {
        return Err(RhythmError::TooLong);
    }
    let step = sixteenth_samples(sample_rate, bpm)?;
    let cells = match usize::try_from((len as i64).div_euclid(step).saturating_add(1)) {
        Ok(n) if n <= 200_000 => n,
        _ => return Err(RhythmError::TooLong),
    };
    let k = 1.0 / channels.len().max(1) as f32;
    let mono: Vec<f32> = (0..len).map(|i| channels.iter().map(|c| c.get(i).copied().unwrap_or(0.0)).sum::<f32>() * k).collect();
    // A hit on the first sample has no earlier frame to differ from, so the flux never sees it.
    // Silence in front gives that hit an onset, and the positions are shifted back afterwards.
    let pad = (sample_rate / 10) as usize;
    let mut padded = vec![0f32; pad + mono.len()];
    padded[pad..].copy_from_slice(&mono);
    let (env, n, hop) = onset_envelope(&padded, sample_rate);
    let mut scores = vec![0f32; cells];
    if hop > 0 {
        for (t, strength) in env.iter().copied().enumerate() {
            // The flux reports an attack a little early. A hit on the first sample can land
            // just before 0; keep it, and drop only what is more than half a 16th early.
            let sample = (t * hop + n) as i64 - 3 * (hop as i64) / 4 - pad as i64;
            if sample < -step / 2 {
                continue;
            }
            let sample = sample.max(0);
            let snapped = snap_earlier(sample, step);
            let cell = usize::try_from(snapped / step).unwrap_or(usize::MAX);
            if let Some(slot) = scores.get_mut(cell) {
                *slot = slot.max(strength);
            }
        }
    }
    let peak = scores.iter().copied().fold(0.0f32, f32::max);
    let mut transients = Vec::new();
    // Half the loudest hit. Quieter than that, a 16th is leakage or a hat under the beat.
    let keep = peak * 0.5;
    if peak >= 0.5 {
        for (i, score) in scores.iter().copied().enumerate() {
            let prev = if i == 0 { 0.0 } else { scores[i - 1] };
            let next = scores.get(i + 1).copied().unwrap_or(0.0);
            if score >= keep && score >= prev && score > next {
                transients.push(Transient { sample: step * i as i64, strength: score });
            }
        }
    }
    Ok(TransientMap { sample_rate, bpm, step, transients })
}

/// One 16th note, in samples, at `bpm`.
pub fn sixteenth_samples(sample_rate: u32, bpm: f64) -> Result<i64, RhythmError> {
    if sample_rate == 0 || sample_rate > MAX_ANALYSIS_SAMPLE_RATE {
        return Err(RhythmError::InvalidSampleRate);
    }
    if !bpm.is_finite() || !(1.0..=999.0).contains(&bpm) {
        return Err(RhythmError::InvalidTempo);
    }
    Ok(((sample_rate as f64) * 60.0 / bpm / 4.0).round().max(1.0) as i64)
}

/// Loudness and spectrum of `channels`, split where either one changes.
///
/// Each window is one quarter note of `bpm`. Windows within [`SECTION_MERGE_LU`] and with the
/// same spectral fullness stay together. Loudness is K-weighted block loudness. Fullness is
/// bass (below 200 Hz), mids, and treble (above 4 kHz), leveled per octave so a full mix is not
/// punished for having less treble than bass. A band 12 dB below the others makes the stretch
/// low energy even when it is loud. The same audio and tempo always give the same sections.
pub fn map_sections(channels: &[&[f32]], sample_rate: u32, bpm: f64) -> Result<Vec<Section>, RhythmError> {
    let (len, step) = checked(channels, sample_rate, bpm)?;
    let beat = step.saturating_mul(4).max(1);
    let rate = f64::from(sample_rate);
    let pre_c = k_prefilter(rate);
    let rlb_c = k_rlb(rate);
    let mut pre = Biquad::new();
    let mut rlb = Biquad::new();
    let mut bands = BandSplit::new(rate);
    let k = 1.0 / channels.len().max(1) as f32;
    let mut windows = Vec::new();
    let mut window = Stretch::new(0);
    for i in 0..len {
        let sample = f64::from(channels.iter().map(|c| c.get(i).copied().unwrap_or(0.0)).sum::<f32>() * k);
        let y = rlb.tick(&rlb_c, pre.tick(&pre_c, sample));
        let (bass, mid, treble) = bands.tick(sample);
        window.n += 1;
        window.k_sum += y * y;
        window.bass += bass * bass;
        window.mid += mid * mid;
        window.treble += treble * treble;
        if window.n == beat || i + 1 == len {
            window.end = i64::try_from(i).unwrap_or(i64::MAX).saturating_add(1);
            let next = window.end;
            windows.push(window);
            window = Stretch::new(next);
        }
    }
    let mut merged: Vec<Stretch> = Vec::new();
    for w in windows {
        if let Some(last) = merged.last_mut()
            && can_merge(last, &w, sample_rate)
        {
            last.add(&w);
        } else {
            merged.push(w);
        }
    }
    Ok(with_intensity(merged, sample_rate))
}

/// Where the videos change, in samples, including `0` and `len`.
///
/// `density` is 0..=1 and `note_steps` is how many 16ths the selected note lasts (1, 2 or 4).
/// At 1 the cuts are that note, every time. Below 1 the aim is `1/density` notes (at most 8), and
/// each cut lands on the hit nearest that aim. A tie stays on the earlier hit. With no hit after
/// the cursor, the cut stays on the aim.
pub fn plan_cuts(hits: &[i64], step: i64, density: f64, note_steps: i64, len: i64) -> Vec<i64> {
    let step = step.max(1);
    let note_steps = note_steps.max(1);
    let note = note_steps.saturating_mul(step);
    let len = len.max(0);
    if len == 0 {
        return vec![0];
    }
    let density = if density.is_finite() { density.clamp(0.0, 1.0) } else { 1.0 };
    if density >= 1.0 {
        return grid(note, len);
    }
    let notes = if density <= 0.0 { 8.0 } else { (1.0 / density).clamp(1.0, 8.0) };
    let span = (notes * note_steps as f64).round().max(note_steps as f64) as i64 * step;
    let mut cuts = vec![0i64];
    let mut cursor = 0i64;
    let limit = usize::try_from(len / step).unwrap_or(20_000).saturating_add(2).min(20_000);
    while cursor < len && cuts.len() < limit {
        let target = cursor.saturating_add(span.max(step));
        if target >= len {
            cuts.push(len);
            break;
        }
        let end = nearest_hit(hits, cursor, target).unwrap_or(target).min(len);
        if end <= cursor {
            cuts.push(len);
            break;
        }
        cuts.push(end);
        cursor = end;
    }
    if cuts.last().copied().unwrap_or(0) < len {
        cuts.push(len);
    }
    cuts
}

/// [`plan_cuts`], with `energy` (0..=1) scaling the hold by each section's intensity.
///
/// Zero energy does not read `sections`: the hold is the transient density alone. Above zero,
/// a section's loudness picks a hold from one note (loudest) to eight notes (quietest), and
/// density pulls that hold back toward one note. Both at 1 cut every selected note, including
/// through a quiet section. A hold of one note stays on the note grid. A longer hold ends on
/// the hit nearest the aim, and a section boundary ends the clip so the next stretch can use
/// its own intensity.
pub fn plan_cuts_with_energy(hits: &[i64], step: i64, density: f64, energy: f64, sections: &[Section], note_steps: i64, len: i64) -> Vec<i64> {
    let energy = if energy.is_finite() { energy.clamp(0.0, 1.0) } else { 0.0 };
    let density = if density.is_finite() { density.clamp(0.0, 1.0) } else { 1.0 };
    if energy <= 0.0 || sections.is_empty() || density >= 1.0 {
        return plan_cuts(hits, step, density, note_steps, len);
    }
    let step = step.max(1);
    let note_steps = note_steps.max(1);
    let note = note_steps.saturating_mul(step);
    let len = len.max(0);
    if len == 0 {
        return vec![0];
    }
    let mut cuts = vec![0i64];
    let mut cursor = 0i64;
    let limit = usize::try_from(len / step).unwrap_or(20_000).saturating_add(2).min(20_000);
    while cursor < len && cuts.len() < limit {
        let section_end = sections.iter().find(|s| s.end > cursor).map(|s| s.end.min(len)).unwrap_or(len);
        if section_end <= cursor {
            break;
        }
        let notes = target_notes(density, energy, intensity_at(sections, cursor));
        if notes <= 1.0 + 1e-6 {
            let next = next_grid_after(cursor, note).min(section_end);
            if next <= cursor {
                break;
            }
            cuts.push(next);
            cursor = next;
            continue;
        }
        let span = (notes * note_steps as f64).round().max(note_steps as f64) as i64 * step;
        let target = cursor.saturating_add(span.max(step));
        if target >= section_end {
            cuts.push(section_end);
            cursor = section_end;
            continue;
        }
        let end = nearest_hit_until(hits, cursor, target, section_end).unwrap_or(target).min(section_end);
        if end <= cursor {
            cuts.push(section_end);
            cursor = section_end;
            continue;
        }
        cuts.push(end);
        cursor = end;
    }
    if cuts.last().copied().unwrap_or(0) < len {
        cuts.push(len);
    }
    cuts
}

/// How many selected notes a clip holds.
///
/// Density alone is one note at 1, two at 0.5, and eight at 0. Energy blends that with the
/// section: full energy uses one note on the loudest section and eight on the quietest, then
/// density scales the extra length away. `1/density` is the density-only hold.
fn target_notes(density: f64, energy: f64, intensity: f64) -> f64 {
    let density = if density.is_finite() { density.clamp(0.0, 1.0) } else { 1.0 };
    let energy = if energy.is_finite() { energy.clamp(0.0, 1.0) } else { 0.0 };
    let slow = hold_notes(density);
    if energy <= 0.0 {
        return slow;
    }
    let intensity = if intensity.is_finite() { intensity.clamp(0.0, 1.0) } else { 0.0 };
    let loudness_hold = 1.0 + 7.0 * (1.0 - intensity);
    let scaled = 1.0 + (loudness_hold - 1.0) * (1.0 - density);
    slow + (scaled - slow) * energy
}

fn hold_notes(density: f64) -> f64 {
    if density >= 1.0 {
        1.0
    } else if density <= 0.0 {
        8.0
    } else {
        (1.0 / density).clamp(1.0, 8.0)
    }
}

fn intensity_at(sections: &[Section], sample: i64) -> f64 {
    sections.iter().find(|s| sample >= s.start && sample < s.end).map(|s| s.intensity).unwrap_or(0.0)
}

fn next_grid_after(cursor: i64, note: i64) -> i64 {
    let note = note.max(1);
    let boundary = cursor.div_euclid(note).saturating_mul(note);
    if boundary > cursor { boundary } else { boundary.saturating_add(note) }
}

/// The hit after `cursor` and at or before `end` closest to `target`. A tie stays earlier.
fn nearest_hit_until(hits: &[i64], cursor: i64, target: i64, end: i64) -> Option<i64> {
    hits.iter().copied().filter(|h| *h > cursor && *h <= end).min_by(|a, b| a.abs_diff(target).cmp(&b.abs_diff(target)).then(a.cmp(b)))
}

fn gated_lufs(sum_sq: f64, n: i64) -> f64 {
    if n <= 0 {
        return f64::NEG_INFINITY;
    }
    let lufs = energy_to_lufs(sum_sq / n as f64);
    if lufs.is_finite() && lufs >= SILENCE_LUFS { lufs } else { f64::NEG_INFINITY }
}

fn same_loudness(a: f64, b: f64) -> bool {
    match (a.is_finite(), b.is_finite()) {
        (false, false) => true,
        (true, true) => (a - b).abs() < SECTION_MERGE_LU,
        _ => false,
    }
}

/// One measured stretch: K-weighted energy plus the three bands.
#[derive(Clone, Copy, Debug)]
struct Stretch {
    start: i64,
    end: i64,
    n: i64,
    k_sum: f64,
    bass: f64,
    mid: f64,
    treble: f64,
}

impl Stretch {
    fn new(start: i64) -> Self {
        Stretch { start, end: start, n: 0, k_sum: 0.0, bass: 0.0, mid: 0.0, treble: 0.0 }
    }

    fn add(&mut self, other: &Stretch) {
        self.end = other.end;
        self.n += other.n;
        self.k_sum += other.k_sum;
        self.bass += other.bass;
        self.mid += other.mid;
        self.treble += other.treble;
    }

    fn lufs(&self) -> f64 {
        gated_lufs(self.k_sum, self.n)
    }
}

fn can_merge(a: &Stretch, b: &Stretch, sample_rate: u32) -> bool {
    same_loudness(a.lufs(), b.lufs()) && (spectral_fullness(a, sample_rate) - spectral_fullness(b, sample_rate)).abs() < 0.34
}

/// Two cascaded biquads, so a band falls about 24 dB an octave past its edge.
struct BandSplit {
    bass: [Biquad; 2],
    bass_c: Coeffs,
    mid_hp: [Biquad; 2],
    mid_lp: [Biquad; 2],
    mid_hp_c: Coeffs,
    mid_lp_c: Coeffs,
    treble: [Biquad; 2],
    treble_c: Coeffs,
}

impl BandSplit {
    fn new(sample_rate: f64) -> Self {
        let (bass_hz, treble_hz, _, _, _) = band_layout(sample_rate);
        let q = 0.707;
        BandSplit {
            bass: [Biquad::new(); 2],
            bass_c: Coeffs::design(FilterType::LowPass, sample_rate, bass_hz, q, 0.0),
            mid_hp: [Biquad::new(); 2],
            mid_lp: [Biquad::new(); 2],
            mid_hp_c: Coeffs::design(FilterType::HighPass, sample_rate, bass_hz, q, 0.0),
            mid_lp_c: Coeffs::design(FilterType::LowPass, sample_rate, treble_hz, q, 0.0),
            treble: [Biquad::new(); 2],
            treble_c: Coeffs::design(FilterType::HighPass, sample_rate, treble_hz, q, 0.0),
        }
    }

    fn tick(&mut self, x: f64) -> (f64, f64, f64) {
        let bass = tick2(&mut self.bass, &self.bass_c, x);
        let mid = tick2(&mut self.mid_lp, &self.mid_lp_c, tick2(&mut self.mid_hp, &self.mid_hp_c, x));
        let treble = tick2(&mut self.treble, &self.treble_c, x);
        (bass, mid, treble)
    }
}

fn tick2(stages: &mut [Biquad; 2], c: &Coeffs, x: f64) -> f64 {
    let y = stages[0].tick(c, x);
    stages[1].tick(c, y)
}

/// Bass edge, treble edge, and the octave width of each band at `sample_rate`.
fn band_layout(sample_rate: f64) -> (f64, f64, f64, f64, f64) {
    let nyquist = sample_rate * 0.5;
    let bass_hz = BASS_HZ.min(nyquist * 0.25).max(40.0);
    let treble_hz = TREBLE_HZ.min(nyquist * 0.7).max(bass_hz * 2.0);
    let bass_oct = (bass_hz / 40.0).log2().max(0.5);
    let mid_oct = (treble_hz / bass_hz).log2().max(0.5);
    let treble_oct = ((nyquist * 0.9) / treble_hz).log2().max(0.5);
    (bass_hz, treble_hz, bass_oct, mid_oct, treble_oct)
}

/// 1 when bass, mids, and treble are in proportion. 0 when any band is missing or far quieter.
fn spectral_fullness(stretch: &Stretch, sample_rate: u32) -> f64 {
    let (_, _, bass_oct, mid_oct, treble_oct) = band_layout(f64::from(sample_rate));
    let bass = band_db(stretch.bass, stretch.n, bass_oct);
    let mid = band_db(stretch.mid, stretch.n, mid_oct);
    let treble = band_db(stretch.treble, stretch.n, treble_oct);
    let Some(gap) = band_gap(bass, mid, treble) else { return 0.0 };
    if gap <= BAND_GAP_FULL_DB {
        1.0
    } else if gap >= BAND_GAP_THIN_DB {
        0.0
    } else {
        (BAND_GAP_THIN_DB - gap) / (BAND_GAP_THIN_DB - BAND_GAP_FULL_DB)
    }
}

fn band_db(sum_sq: f64, n: i64, octaves: f64) -> f64 {
    if n <= 0 || !octaves.is_finite() || octaves <= 0.0 || !sum_sq.is_finite() || sum_sq <= 0.0 {
        return f64::NEG_INFINITY;
    }
    let per_octave = sum_sq / (n as f64 * octaves);
    if per_octave <= 1e-20 { f64::NEG_INFINITY } else { 10.0 * per_octave.log10() }
}

/// Loudest band minus quietest, in dB. None when any band is silent.
fn band_gap(bass: f64, mid: f64, treble: f64) -> Option<f64> {
    if !bass.is_finite() || !mid.is_finite() || !treble.is_finite() {
        return None;
    }
    let max = bass.max(mid).max(treble);
    let min = bass.min(mid).min(treble);
    Some(max - min)
}

/// Loudness used for intensity. A thin spectrum is treated as silence, however loud it is.
fn effective_lufs(stretch: &Stretch, sample_rate: u32) -> f64 {
    let lufs = stretch.lufs();
    let full = spectral_fullness(stretch, sample_rate);
    if !lufs.is_finite() || full <= 0.0 {
        f64::NEG_INFINITY
    } else if full >= 1.0 {
        lufs
    } else {
        SILENCE_LUFS + (lufs - SILENCE_LUFS) * full
    }
}

fn with_intensity(merged: Vec<Stretch>, sample_rate: u32) -> Vec<Section> {
    let effective: Vec<f64> = merged.iter().map(|s| effective_lufs(s, sample_rate)).collect();
    let audible: Vec<f64> = effective.iter().copied().filter(|l| l.is_finite()).collect();
    let any_silence = effective.iter().any(|l| !l.is_finite());
    let (min, max) = audible.iter().copied().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), l| (lo.min(l), hi.max(l)));
    let span = if min.is_finite() { max - min } else { 0.0 };
    // Silence, and a loud stretch that is missing a band, sit at the bottom. When the file also
    // has one of those, a full-spectrum stretch is measured up from the absolute gate.
    let floor = if any_silence { SILENCE_LUFS } else { min };
    let height = if max.is_finite() { max - floor } else { 0.0 };
    merged
        .into_iter()
        .zip(effective)
        .map(|(stretch, level)| {
            let intensity = if !level.is_finite() || height < 1.0 {
                if level.is_finite() && any_silence { 1.0 } else { 0.0 }
            } else if any_silence {
                ((level - floor) / height).clamp(0.0, 1.0)
            } else {
                ((level - min) / span).clamp(0.0, 1.0)
            };
            Section { start: stretch.start, end: stretch.end, lufs: stretch.lufs(), intensity }
        })
        .collect()
}

fn checked(channels: &[&[f32]], sample_rate: u32, bpm: f64) -> Result<(usize, i64), RhythmError> {
    if sample_rate == 0 || sample_rate > MAX_ANALYSIS_SAMPLE_RATE {
        return Err(RhythmError::InvalidSampleRate);
    }
    if !bpm.is_finite() || !(1.0..=999.0).contains(&bpm) {
        return Err(RhythmError::InvalidTempo);
    }
    let len = channels.iter().map(|c| c.len()).max().unwrap_or(0);
    if len == 0 {
        return Err(RhythmError::Empty);
    }
    if (len as u64) > u64::from(sample_rate).saturating_mul(MAX_ANALYSIS_SECONDS) {
        return Err(RhythmError::TooLong);
    }
    let step = sixteenth_samples(sample_rate, bpm)?;
    Ok((len, step))
}

fn grid(note: i64, len: i64) -> Vec<i64> {
    let mut cuts = vec![0i64];
    let mut t = note.max(1);
    while t < len && cuts.len() < 20_000 {
        cuts.push(t);
        t = t.saturating_add(note.max(1));
    }
    if cuts.last().copied().unwrap_or(0) < len {
        cuts.push(len);
    }
    cuts
}

/// The hit strictly after `cursor` closest to `target`. A tie stays on the earlier hit.
fn nearest_hit(hits: &[i64], cursor: i64, target: i64) -> Option<i64> {
    hits.iter().copied().filter(|h| *h > cursor).min_by(|a, b| (a - target).abs().cmp(&(b - target).abs()).then(a.cmp(b)))
}

/// Nearest multiple of `step`. A tie stays on the earlier one. `step` is positive.
fn snap_earlier(sample: i64, step: i64) -> i64 {
    let base = sample.div_euclid(step).saturating_mul(step);
    let next = base.saturating_add(step);
    if sample.saturating_sub(base) <= next.saturating_sub(sample) { base } else { next }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn burst(x: &mut [f32], at: usize, seed: u32) {
        let mut rng = seed;
        let n = x.len().saturating_sub(at).min(800);
        for k in 0..n {
            rng = rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = (rng >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0;
            let env = (-(k as f32) / 80.0).exp();
            x[at + k] += 0.9 * env * noise;
        }
    }

    fn add_tone(x: &mut [f32], from: usize, to: usize, freq: f32, amp: f32, sr: u32) {
        let to = to.min(x.len());
        for i in from..to {
            let t = i as f32 / sr as f32;
            if let Some(sample) = x.get_mut(i) {
                *sample += amp * (t * freq * std::f32::consts::TAU).sin();
            }
        }
    }

    #[test]
    fn hits_land_on_the_beats_and_on_a_loud_offbeat() {
        let sr = 22_050u32;
        let bpm = 120.0;
        let step = ((sr as f64) * 60.0 / bpm / 4.0).round() as i64;
        let beat = step * 4;
        // four beats, plus the "and" of beat 2 as loud as the others
        let mut x = vec![0f32; (beat * 4 + step) as usize];
        for (i, at) in [0, beat, beat * 2, beat * 3].into_iter().enumerate() {
            burst(&mut x, at as usize, 10 + i as u32);
        }
        burst(&mut x, (beat + beat / 2) as usize, 99);
        let map = map_transients(&[&x], sr, bpm).expect("map");
        let got: Vec<i64> = map.transients.iter().map(|t| t.sample).collect();
        let expect = [0, beat, beat + beat / 2, beat * 2, beat * 3];
        assert_eq!(got, expect, "step {step}");
        assert!(map_transients(&[&x], sr, 0.0).is_err());
        assert!(map_transients(&[&[]], sr, bpm).is_err());
        let silent = vec![0f32; x.len()];
        assert!(map_transients(&[&silent], sr, bpm).expect("silence").transients.is_empty());
    }

    #[test]
    fn full_density_is_the_note_and_the_midpoint_ends_on_a_hit() {
        let step = 10i64;
        let len = 160; // four quarter notes
        let quarters = plan_cuts(&[], step, 1.0, 4, len);
        assert_eq!(quarters, vec![0, 40, 80, 120, 160]);
        // hits on beats 0, 1 and 4, plus one at 2.5 beats (sample 100)
        let hits = [0, 40, 100, 160];
        let mid = plan_cuts(&hits, step, 0.5, 4, len);
        assert_eq!(mid, vec![0, 100, 160], "two quarter notes would land at 80; the nearest hit is 100");
        let loud = [Section { start: 0, end: len, lufs: -8.0, intensity: 1.0 }];
        let ignored = plan_cuts_with_energy(&hits, step, 0.5, 0.0, &loud, 4, len);
        assert_eq!(ignored, mid, "zero energy does not read the section");
    }

    #[test]
    fn full_energy_and_full_density_is_the_note_and_a_loud_section_cuts_faster() {
        let step = 10i64;
        let len = 320; // eight quarter notes
        let hits: Vec<i64> = (0..=8).map(|i| i * 40).chain([50]).collect();
        let sections = [Section { start: 0, end: 160, lufs: f64::NEG_INFINITY, intensity: 0.0 }, Section { start: 160, end: 320, lufs: -8.0, intensity: 1.0 }];
        let strict = plan_cuts_with_energy(&hits, step, 1.0, 1.0, &sections, 4, len);
        assert_eq!(strict, vec![0, 40, 80, 120, 160, 200, 240, 280, 320], "both at full ignore the off-grid hit");
        // density 0.5 and energy 1: the quiet stretch aims past its own end, so it is one clip;
        // the loud stretch is one note, on the grid, not on the hit at sample 50.
        let mixed = plan_cuts_with_energy(&hits, step, 0.5, 1.0, &sections, 4, len);
        assert_eq!(mixed, vec![0, 160, 200, 240, 280, 320]);
        let quiet_only = [Section { start: 0, end: len, lufs: -24.0, intensity: 0.0 }];
        let fine: Vec<i64> = (0..=32).map(|i| i * step).collect();
        let held = plan_cuts_with_energy(&fine, step, 0.5, 1.0, &quiet_only, 4, len);
        assert_eq!(held, vec![0, 180, 320], "a quiet stretch at mid density aims at 4.5 quarter notes");
    }

    #[test]
    fn silence_then_a_full_spectrum_is_loud_and_a_tone_is_not() {
        let sr = 22_050u32;
        let bpm = 120.0;
        let beat = sixteenth_samples(sr, bpm).expect("step") * 4;
        let mut full = vec![0f32; (beat * 8) as usize];
        let end = full.len();
        let mid = (beat * 4) as usize;
        add_tone(&mut full, mid, end, 80.0, 0.2, sr);
        add_tone(&mut full, mid, end, 1_000.0, 0.2, sr);
        add_tone(&mut full, mid, end, 6_000.0, 0.2, sr);
        let sections = map_sections(&[&full], sr, bpm).expect("sections");
        assert!(sections.len() >= 2, "{sections:?}");
        assert_eq!(sections[0].start, 0);
        assert!(sections[0].intensity < 0.05, "{sections:?}");
        assert!(sections.last().expect("full").intensity > 0.9, "{sections:?}");

        // Each single tone is louder than the full mix, and each is missing the other bands.
        for freq in [70.0, 1_000.0, 8_000.0] {
            let mut thin = vec![0f32; full.len()];
            add_tone(&mut thin, 0, mid, freq, 0.7, sr);
            add_tone(&mut thin, mid, end, 80.0, 0.2, sr);
            add_tone(&mut thin, mid, end, 1_000.0, 0.2, sr);
            add_tone(&mut thin, mid, end, 6_000.0, 0.2, sr);
            let sections = map_sections(&[&thin], sr, bpm).expect("thin");
            let at = (mid as i64) - 1;
            let tone = sections.iter().find(|s| s.start <= at && at < s.end).expect("tone stretch");
            let wide = sections.last().expect("full end");
            assert!(tone.lufs > wide.lufs, "{freq} Hz is louder but thin: {sections:?}");
            assert!(tone.intensity < 0.25, "{freq} Hz is missing a band: {sections:?}");
            assert!(wide.intensity > 0.8, "bass, mids, and treble together are full on: {sections:?}");
        }

        let mut flat = vec![0f32; full.len()];
        add_tone(&mut flat, 0, end, 440.0, 0.2, sr);
        let one = map_sections(&[&flat], sr, bpm).expect("flat");
        assert!(one.iter().all(|s| s.intensity == 0.0), "a tone has no full-spectrum contrast: {one:?}");
        assert!(map_sections(&[&[]], sr, bpm).is_err());
    }
}
