//! Verification tool, not a user-facing feature.
//!
//! Prints what the scanner thinks the collection is, so the library layer can be
//! checked against reality before any UI exists to look at.
//!
//!     cargo run --bin scan_report -- ~/music

use std::collections::BTreeMap;
use std::time::Instant;

use hark_lib::library::{self, AlbumKind, ArtSource};

fn main() {
    let arg = std::env::args().nth(1);
    let root = match library::resolve_music_dir(arg.as_deref()) {
        Some(r) => r,
        None => {
            eprintln!("no music directory could be resolved");
            std::process::exit(1);
        }
    };

    println!("root: {}", root.display());
    if !root.exists() {
        println!("(does not exist — the grid would come up empty and prompt for a path)");
        return;
    }

    let started = Instant::now();
    let lib = library::scan::scan(&root);
    let elapsed = started.elapsed();

    let folders = lib
        .albums
        .iter()
        .filter(|a| a.kind == AlbumKind::Folder)
        .count();
    let singles = lib
        .albums
        .iter()
        .filter(|a| a.kind == AlbumKind::Singles)
        .count();
    let tracks: usize = lib.albums.iter().map(|a| a.tracks.len()).sum();
    let perfect = lib.albums.iter().filter(|a| a.perfect).count();

    let art_file = lib
        .albums
        .iter()
        .filter(|a| matches!(a.art, ArtSource::File(_)))
        .count();
    let art_embedded = lib
        .albums
        .iter()
        .filter(|a| matches!(a.art, ArtSource::Embedded(_)))
        .count();
    let art_none: Vec<_> = lib
        .albums
        .iter()
        .filter(|a| matches!(a.art, ArtSource::None))
        .collect();

    println!("scan took {:?}", elapsed);
    println!();
    println!("albums (folders):  {folders}");
    println!("singles tiles:     {singles}");
    println!("total tiles:       {}", lib.albums.len());
    println!("tracks:            {tracks}");
    println!("marked perfect:    {perfect}");
    println!();
    println!("art from cover file:  {art_file}");
    println!("art from embedded:    {art_embedded}");
    println!("no art at all:        {}", art_none.len());

    // The Björk check: if NFC normalization is working, there is exactly one.
    let mut by_artist: BTreeMap<&str, usize> = BTreeMap::new();
    for album in &lib.albums {
        *by_artist.entry(album.artist.as_str()).or_default() += 1;
    }
    println!("distinct artists:     {}", by_artist.len());
    let bjorks: Vec<_> = by_artist
        .keys()
        .filter(|a| a.to_lowercase().starts_with("bj"))
        .collect();
    println!("artists starting 'Bj': {bjorks:?}");

    println!();
    println!("--- albums with no art ({}) ---", art_none.len());
    for album in &art_none {
        println!("  {} — {}", album.artist, album.title);
    }

    println!();
    println!("--- singles tiles ({singles}) ---");
    for album in lib.albums.iter().filter(|a| a.kind == AlbumKind::Singles) {
        println!("  {} ({} tracks)", album.artist, album.tracks.len());
    }

    println!();
    println!("--- all tiles in grid order ---");
    for album in lib.albums.iter() {
        println!(
            "  {:<28} {:<40} {}",
            album.artist,
            album.title,
            album.year.map(|y| y.to_string()).unwrap_or_default()
        );
    }
}
