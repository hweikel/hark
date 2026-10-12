//! Verification tool: drive the audio engine with no UI in the way.
//!
//! Proves two things the GUI can't easily show — that a file decodes and
//! reaches the device at all, and that it does so at its own sample rate
//! rather than being resampled on the way.
//!
//!     cargo run --bin play_probe -- "/home/hweikel/music/David Bowie/Blackstar" 4

use std::path::PathBuf;
use std::time::Duration;

use hark_lib::audio::{Command, Player};
use hark_lib::library::decode_spec_of;

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = match args.next() {
        Some(d) => PathBuf::from(d),
        None => {
            eprintln!("usage: play_probe <album-dir> [seconds]");
            std::process::exit(1);
        }
    };
    let seconds: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(4);

    let mut tracks: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| hark_lib::library::tags::is_audio_file(p))
        .collect();
    tracks.sort();

    if tracks.is_empty() {
        eprintln!("no audio files in {}", dir.display());
        std::process::exit(1);
    }

    println!("album:  {}", dir.display());
    println!("tracks: {}", tracks.len());
    match decode_spec_of(&tracks[0]) {
        Ok(spec) => println!(
            "first track decodes as {} Hz, {} ch",
            spec.sample_rate, spec.channels
        ),
        Err(e) => {
            eprintln!("could not open first track: {e}");
            std::process::exit(1);
        }
    }

    let player = Player::spawn(|state| {
        println!(
            "  [event] playing={} index={:?} shuffle={}",
            state.playing, state.index, state.shuffle
        );
    });

    player.send(Command::Load {
        tile_id: "probe".into(),
        tracks,
        start: 0,
    });

    println!("playing for {seconds}s — listen, and check `pw-top` in another shell");
    std::thread::sleep(Duration::from_secs(seconds));

    player.send(Command::Stop);
    std::thread::sleep(Duration::from_millis(300));
    println!("done");
}
