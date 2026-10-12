//! The cpal output stream, opened at the source file's own sample rate.
//!
//! This is the half of "sounds as intended" that hark! controls. The other half
//! is PipeWire: by default its graph runs at 48kHz and quietly resamples the
//! 44.1kHz material that makes up most of this collection. Opening the device
//! at 44100 asks PipeWire to switch the graph rate instead — which it will only
//! do if `default.clock.allowed-rates` lists the rate. See the matching block
//! in /etc/nixos/configuration.nix.

use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, StreamConfig};
use ringbuf::traits::{Consumer, Observer, Split};
use ringbuf::{HeapCons, HeapRb};

use super::decode::StreamSpec;

/// How much decoded audio to keep ahead of the device.
///
/// Deliberately short. The UI advances to the next track when the *decoder*
/// finishes, so this buffer is also the amount by which the highlighted track
/// can run ahead of what you hear. 250ms is inaudible as a lead and still an
/// enormous margin against underrun on this hardware.
const BUFFER_MS: u32 = 250;

pub struct Output {
    _stream: cpal::Stream,
    pub spec: StreamSpec,
    pub producer: ringbuf::HeapProd<f32>,
    /// True once the device has pulled everything the decoder wrote, which is
    /// how the engine knows a finished track has actually finished playing.
    pub drained: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Output {
    pub fn open(spec: StreamSpec) -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| anyhow!("no default audio output device"))?;

        let capacity =
            (spec.sample_rate as usize * spec.channels as usize * BUFFER_MS as usize) / 1000;
        let (producer, mut consumer) = HeapRb::<f32>::new(capacity.max(4096)).split();

        let config = StreamConfig {
            channels: spec.channels,
            // cpal 0.18 made SampleRate a plain u32 type alias.
            sample_rate: spec.sample_rate,
            buffer_size: cpal::BufferSize::Default,
        };

        let drained = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let drained_for_cb = drained.clone();

        // f32 is PipeWire's own internal format, so this is the one output
        // format that involves no conversion at all.
        let stream = device.build_output_stream(
            config,
            move |out: &mut [f32], _| fill(out, &mut consumer, &drained_for_cb),
            |err| eprintln!("hark: audio output error: {err}"),
            None,
        )?;

        stream.play()?;

        Ok(Output {
            _stream: stream,
            spec,
            producer,
            drained,
        })
    }

    /// Whether the device supports a given spec without resampling.
    ///
    /// Reported for diagnostics; cpal will still try, and PipeWire accepts
    /// essentially anything by resampling if it must.
    pub fn supports_natively(spec: StreamSpec) -> bool {
        let Some(device) = cpal::default_host().default_output_device() else {
            return false;
        };
        let Ok(configs) = device.supported_output_configs() else {
            return false;
        };
        configs
            .filter(|c| c.sample_format() == SampleFormat::F32)
            .filter(|c| c.channels() == spec.channels)
            .any(|c| {
                c.min_sample_rate() <= spec.sample_rate && spec.sample_rate <= c.max_sample_rate()
            })
    }
}

fn fill(
    out: &mut [f32],
    consumer: &mut HeapCons<f32>,
    drained: &std::sync::atomic::AtomicBool,
) {
    let taken = consumer.pop_slice(out);
    if taken < out.len() {
        // Underrun, or the end of the last track. Silence rather than garbage.
        out[taken..].fill(0.0);
    }
    drained.store(
        consumer.occupied_len() == 0,
        std::sync::atomic::Ordering::Relaxed,
    );
}
