//! The playback engine: one thread owning the decoder and the output device.
//!
//! Scope is deliberately small, matching what the now-playing row actually
//! offers: play/pause, previous/next, jump to a track, shuffle. There is no
//! queue — you play an album or a playlist, and when it runs out it stops.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use rand::seq::SliceRandom;
use serde::Serialize;

use super::decode::TrackDecoder;
use super::output::Output;

pub enum Command {
    /// Start a tile (album or playlist) at a given track.
    Load {
        tile_id: String,
        tracks: Vec<PathBuf>,
        start: usize,
    },
    Toggle,
    Next,
    Previous,
    PlayIndex(usize),
    SetShuffle(bool),
    Stop,
}

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackState {
    /// The tile currently loaded, or `None` before anything has been played —
    /// which is how the frontend knows not to draw the now-playing row at all.
    pub tile_id: Option<String>,
    /// Index into that tile's track list.
    pub index: Option<usize>,
    pub playing: bool,
    pub shuffle: bool,
}

pub struct Player {
    commands: Sender<Command>,
    state: Arc<Mutex<PlaybackState>>,
}

impl Player {
    /// Spawn the engine thread. `on_change` is called whenever the state the
    /// frontend cares about changes.
    pub fn spawn(on_change: impl Fn(PlaybackState) + Send + 'static) -> Player {
        let (tx, rx) = mpsc::channel();
        let state = Arc::new(Mutex::new(PlaybackState::default()));
        let engine_state = state.clone();

        std::thread::Builder::new()
            .name("hark-audio".into())
            .spawn(move || Engine::new(rx, engine_state, Box::new(on_change)).run())
            .expect("failed to spawn audio thread");

        Player {
            commands: tx,
            state,
        }
    }

    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }

    /// Follow a tile whose id changed underneath playback — a renamed
    /// playlist — so the now-playing row doesn't lose it.
    pub fn rename_tile(&self, from: &str, to: &str) {
        if let Ok(mut state) = self.state.lock() {
            if state.tile_id.as_deref() == Some(from) {
                state.tile_id = Some(to.to_string());
            }
        }
    }

    pub fn state(&self) -> PlaybackState {
        self.state.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

struct Engine {
    commands: Receiver<Command>,
    state: Arc<Mutex<PlaybackState>>,
    on_change: Box<dyn Fn(PlaybackState) + Send>,

    tracks: Vec<PathBuf>,
    /// Play order as indices into `tracks`. Sequential normally, permuted when
    /// shuffle is on.
    order: Vec<usize>,
    position: usize,

    output: Option<Output>,
    decoder: Option<TrackDecoder>,
    decoder_finished: bool,
    playing: bool,
    shuffle: bool,
}

impl Engine {
    fn new(
        commands: Receiver<Command>,
        state: Arc<Mutex<PlaybackState>>,
        on_change: Box<dyn Fn(PlaybackState) + Send>,
    ) -> Self {
        Engine {
            commands,
            state,
            on_change,
            tracks: Vec::new(),
            order: Vec::new(),
            position: 0,
            output: None,
            decoder: None,
            decoder_finished: false,
            playing: false,
            shuffle: false,
        }
    }

    fn run(mut self) {
        loop {
            let mut got_command = false;
            loop {
                match self.commands.try_recv() {
                    Ok(command) => {
                        got_command = true;
                        self.handle(command);
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => return,
                }
            }

            let worked = self.pump();

            if !worked && !got_command {
                // Nothing to decode and no commands: idle cheaply.
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }

    fn handle(&mut self, command: Command) {
        match command {
            Command::Load {
                tile_id,
                tracks,
                start,
            } => {
                self.tracks = tracks;
                self.rebuild_order(start);
                self.set_tile(Some(tile_id));
                self.open_current();
            }
            Command::Toggle => {
                if self.decoder.is_some() {
                    self.playing = !self.playing;
                    self.publish();
                }
                // With nothing loaded, space does nothing — as specified.
            }
            Command::Next => self.advance(1),
            Command::Previous => self.advance(-1),
            Command::PlayIndex(index) => {
                if index < self.tracks.len() {
                    self.position = self
                        .order
                        .iter()
                        .position(|&t| t == index)
                        .unwrap_or(self.position);
                    self.open_current();
                }
            }
            Command::SetShuffle(on) => {
                self.shuffle = on;
                // Keep playing whatever is playing; only what comes *after* it
                // changes. Turning shuffle off resumes the album in order from
                // the current song rather than jumping.
                let current = self.current_track_index().unwrap_or(0);
                self.rebuild_order(current);
                self.publish();
            }
            Command::Stop => self.stop(),
        }
    }

    /// Build the play order, placing `current` first.
    fn rebuild_order(&mut self, current: usize) {
        let count = self.tracks.len();
        if count == 0 {
            self.order.clear();
            self.position = 0;
            return;
        }
        let current = current.min(count - 1);

        if self.shuffle {
            let mut rest: Vec<usize> = (0..count).filter(|&i| i != current).collect();
            rest.shuffle(&mut rand::rng());
            self.order = std::iter::once(current).chain(rest).collect();
            self.position = 0;
        } else {
            self.order = (0..count).collect();
            self.position = current;
        }
    }

    fn current_track_index(&self) -> Option<usize> {
        self.order.get(self.position).copied()
    }

    fn open_current(&mut self) {
        let Some(track_index) = self.current_track_index() else {
            self.stop();
            return;
        };
        let Some(path) = self.tracks.get(track_index).cloned() else {
            self.stop();
            return;
        };

        match TrackDecoder::open(&path) {
            Ok(decoder) => {
                // Rebuild the device only when the format actually changes.
                // This collection mixes 44.1/16 with 48k, 88.2k and 96k, so
                // that happens for real.
                let needs_new_output = match &self.output {
                    Some(output) => output.spec != decoder.spec,
                    None => true,
                };
                if needs_new_output {
                    if !Output::supports_natively(decoder.spec) {
                        eprintln!(
                            "hark: {} Hz not offered natively by the output device; \
                             PipeWire will resample. Check default.clock.allowed-rates.",
                            decoder.spec.sample_rate
                        );
                    }
                    match Output::open(decoder.spec) {
                        Ok(output) => self.output = Some(output),
                        Err(e) => {
                            eprintln!("hark: could not open audio output: {e}");
                            self.stop();
                            return;
                        }
                    }
                }
                self.decoder = Some(decoder);
                self.decoder_finished = false;
                self.playing = true;
                self.publish();
            }
            Err(e) => {
                eprintln!("hark: cannot decode {}: {e}", path.display());
                // Skip the unplayable file rather than stalling the album.
                self.advance(1);
            }
        }
    }

    fn advance(&mut self, delta: isize) {
        if self.order.is_empty() {
            return;
        }
        let next = self.position as isize + delta;
        if next < 0 {
            // Previous on the first track restarts it, which is what every
            // other player does and what the hand expects.
            self.position = 0;
            self.open_current();
            return;
        }
        if next as usize >= self.order.len() {
            // "if a playlist or album runs out, it simply stops playing"
            self.stop();
            return;
        }
        self.position = next as usize;
        self.open_current();
    }

    fn stop(&mut self) {
        self.playing = false;
        self.decoder = None;
        self.decoder_finished = false;
        // Dropping the output releases the PipeWire node so hark! doesn't sit
        // in the mixer forever holding a silent stream.
        self.output = None;
        self.publish();
    }

    /// Move audio from the decoder into the device. Returns whether it did work.
    fn pump(&mut self) -> bool {
        if !self.playing {
            return false;
        }

        let (Some(output), Some(decoder)) = (self.output.as_mut(), self.decoder.as_mut()) else {
            return false;
        };

        if !self.decoder_finished {
            // Top the ring buffer up. `buffered()` is whatever the previous
            // packet left over.
            loop {
                let buffered = decoder.buffered();
                if buffered.is_empty() {
                    match decoder.decode_next() {
                        Ok(true) => continue,
                        Ok(false) => {
                            self.decoder_finished = true;
                            break;
                        }
                        Err(e) => {
                            eprintln!("hark: decode error: {e}");
                            self.decoder_finished = true;
                            break;
                        }
                    }
                }

                let buffered = decoder.buffered();
                let written = {
                    use ringbuf::traits::Producer;
                    output.producer.push_slice(buffered)
                };
                decoder.consume(written);

                if written == 0 {
                    // Device buffer is full; come back in a moment.
                    return false;
                }
            }
        }

        if self.decoder_finished {
            // Wait for the device to actually play out what we already wrote
            // before moving on, so the highlighted track matches what's audible.
            let drained = output
                .drained
                .load(std::sync::atomic::Ordering::Relaxed);
            if drained && decoder.buffered().is_empty() {
                self.advance(1);
            }
            return false;
        }

        true
    }

    fn set_tile(&mut self, tile_id: Option<String>) {
        if let Ok(mut state) = self.state.lock() {
            state.tile_id = tile_id;
        }
    }

    fn publish(&self) {
        let snapshot = {
            let Ok(mut state) = self.state.lock() else {
                return;
            };
            state.index = self.current_track_index();
            state.playing = self.playing;
            state.shuffle = self.shuffle;
            if self.decoder.is_none() && !self.playing {
                state.index = self.current_track_index();
            }
            state.clone()
        };
        (self.on_change)(snapshot);
    }
}
