//! Symphonia decoding: one file in, interleaved f32 out.
//!
//! Decoding natively (rather than handing paths to a webview `<audio>` element)
//! is what makes the "sounds as intended" requirement checkable. We know the
//! source sample rate, we open the output at that rate, and nothing resamples
//! or requantizes on the way through.

use std::fs::File;
use std::path::Path;

use anyhow::{anyhow, Result};
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::codecs::CodecParameters;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

/// The audio format of a decoded stream, used to decide whether the output
/// device has to be rebuilt between tracks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamSpec {
    pub sample_rate: u32,
    pub channels: u16,
}

pub struct TrackDecoder {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    pub spec: StreamSpec,
    /// Interleaved f32 decoded but not yet handed to the output. Packet sizes
    /// rarely line up with available ring-buffer space.
    pending: Vec<f32>,
    pending_offset: usize,
}

impl TrackDecoder {
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        let stream = MediaSourceStream::new(Box::new(file), Default::default());

        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }

        let format = symphonia::default::get_probe().probe(
            &hint,
            stream,
            // Symphonia 0.6 applies encoder delay/padding trimming in the
            // format readers themselves, so there is no longer a gapless
            // option to set here.
            FormatOptions::default(),
            MetadataOptions::default(),
        )?;

        let track = format
            .first_track_known_codec(TrackType::Audio)
            .ok_or_else(|| anyhow!("no decodable audio track in {}", path.display()))?;
        let track_id = track.id;

        let params = match &track.codec_params {
            Some(CodecParameters::Audio(params)) => params.clone(),
            _ => return Err(anyhow!("track in {} is not audio", path.display())),
        };

        let sample_rate = params
            .sample_rate
            .ok_or_else(|| anyhow!("unknown sample rate in {}", path.display()))?;
        let channels = params
            .channels
            .as_ref()
            .map(|c| c.count() as u16)
            .ok_or_else(|| anyhow!("unknown channel layout in {}", path.display()))?;

        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(&params, &AudioDecoderOptions::default())?;

        Ok(TrackDecoder {
            format,
            decoder,
            track_id,
            spec: StreamSpec {
                sample_rate,
                channels,
            },
            pending: Vec::new(),
            pending_offset: 0,
        })
    }

    /// Samples decoded but not yet consumed.
    pub fn buffered(&self) -> &[f32] {
        &self.pending[self.pending_offset..]
    }

    pub fn consume(&mut self, count: usize) {
        self.pending_offset += count;
        if self.pending_offset >= self.pending.len() {
            self.pending.clear();
            self.pending_offset = 0;
        }
    }

    /// Decode one more packet into the pending buffer.
    ///
    /// Returns `false` at end of stream. A packet that fails to decode is
    /// skipped rather than ending the track — one damaged frame in the middle
    /// of an album shouldn't stop playback.
    pub fn decode_next(&mut self) -> Result<bool> {
        loop {
            let packet = match self.format.next_packet()? {
                Some(packet) => packet,
                None => return Ok(false),
            };

            if packet.track_id != self.track_id {
                continue;
            }

            match self.decoder.decode(&packet) {
                Ok(decoded) => {
                    if decoded.is_empty() {
                        continue;
                    }
                    if self.pending_offset > 0 {
                        self.pending.drain(..self.pending_offset);
                        self.pending_offset = 0;
                    }
                    // f32 interleaved is exactly what cpal wants and what
                    // PipeWire uses internally — no further conversion.
                    decoded.copy_to_vec_interleaved(&mut self.pending);
                    return Ok(true);
                }
                Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
                Err(symphonia::core::errors::Error::ResetRequired) => return Ok(false),
                Err(e) => return Err(e.into()),
            }
        }
    }
}
