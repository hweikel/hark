//! Cover art: extraction, downscaling, and a session-only memory cache.
//!
//! The important decision here is that the webview never sees a full-size
//! image. Covers in this collection run to 1200×1200; decoded in the webview
//! that is 5.7MB of bitmap each, so twenty visible tiles would cost >100MB of
//! renderer memory and the whole grid would cost gigabytes. Everything is
//! decoded and resized in Rust and handed over at tile size.
//!
//! Nothing is written to disk. The cache lives for the life of the process,
//! which is what "nothing cached between sessions" asks for — relaunching
//! re-reads the collection and that is the intended behaviour.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use anyhow::{anyhow, Result};
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{ImageReader, Rgb, RgbImage};
use lofty::config::ParseOptions;
use lofty::file::TaggedFileExt;
use lofty::probe::Probe;

use super::scan::ArtSource;

/// Quality for the re-encoded tile image. 88 is visually indistinguishable from
/// the source at tile size and roughly a tenth the bytes.
const JPEG_QUALITY: u8 = 88;

/// Sizes the frontend is allowed to ask for. Bounded so a crafted request can't
/// make hark! allocate an arbitrarily large buffer, and so the cache can't be
/// blown up by requesting 10,000 distinct sizes.
const ALLOWED_SIZES: &[u32] = &[180, 360, 540];

/// `--ink-raised` from the stylesheet, so an incomplete mosaic's empty
/// quadrants read as tile background rather than a black hole.
const BACKDROP: Rgb<u8> = Rgb([0x10, 0x14, 0x20]);

pub fn clamp_size(requested: u32) -> u32 {
    ALLOWED_SIZES
        .iter()
        .copied()
        .min_by_key(|s| s.abs_diff(requested))
        .unwrap_or(360)
}

#[derive(Default)]
pub struct ArtCache {
    entries: Mutex<HashMap<(u32, u32), Vec<u8>>>,
}

impl ArtCache {
    pub fn get(&self, art_id: u32, size: u32) -> Option<Vec<u8>> {
        self.entries.lock().ok()?.get(&(art_id, size)).cloned()
    }

    pub fn put(&self, art_id: u32, size: u32, bytes: Vec<u8>) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.insert((art_id, size), bytes);
        }
    }

    /// Drop cached renders for one album, after its art has been replaced.
    pub fn invalidate(&self, art_id: u32) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.retain(|(id, _), _| *id != art_id);
        }
    }

    /// Drop everything. Required after a rescan, which reassigns art handles —
    /// a surviving entry would render the wrong album's cover.
    pub fn clear(&self) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.clear();
        }
    }
}

/// Pull the raw image bytes an `ArtSource` points at.
pub fn raw_bytes(source: &ArtSource) -> Result<Vec<u8>> {
    match source {
        ArtSource::File(path) => {
            Ok(std::fs::read(path).map_err(|e| anyhow!("reading {}: {e}", path.display()))?)
        }
        ArtSource::Embedded(path) => embedded_picture(path),
        ArtSource::Mosaic(_) => Err(anyhow!("a mosaic has no single source image")),
        ArtSource::None => Err(anyhow!("album has no art")),
    }
}

fn embedded_picture(path: &Path) -> Result<Vec<u8>> {
    let tagged = Probe::open(path)?
        // Audio properties are irrelevant here and parsing them means walking
        // frame headers — skipping it makes art extraction noticeably cheaper.
        .options(ParseOptions::new().read_properties(false))
        .read()?;

    let tag = tagged
        .primary_tag()
        .or_else(|| tagged.first_tag())
        .ok_or_else(|| anyhow!("no tag in {}", path.display()))?;

    let picture = tag
        .pictures()
        .first()
        .ok_or_else(|| anyhow!("no embedded picture in {}", path.display()))?;

    Ok(picture.data().to_vec())
}

/// Decode, square-crop, downscale, and re-encode as JPEG.
///
/// Covers are overwhelmingly square already; the centre crop only matters for
/// the handful that aren't, where letterboxing would break the grid's rhythm.
pub fn render_tile(source: &ArtSource, size: u32) -> Result<Vec<u8>> {
    if let ArtSource::Mosaic(parts) = source {
        return render_mosaic(parts, size);
    }
    let bytes = raw_bytes(source)?;
    render_tile_from_bytes(&bytes, size)
}

/// Composite up to four covers into a 2×2 square.
///
/// Quadrants are filled in order; with two or three sources the remainder stays
/// the tile background rather than repeating an image, which would misrepresent
/// what's on the playlist.
fn render_mosaic(parts: &[ArtSource], size: u32) -> Result<Vec<u8>> {
    if parts.is_empty() {
        return Err(anyhow!("empty mosaic"));
    }

    let half = (size / 2).max(1);
    let mut canvas = RgbImage::from_pixel(half * 2, half * 2, BACKDROP);

    let mut placed = 0;
    for (index, part) in parts.iter().take(4).enumerate() {
        let Ok(bytes) = raw_bytes(part) else { continue };
        let Ok(tile) = ImageReader::new(std::io::Cursor::new(&bytes))
            .with_guessed_format()
            .and_then(|r| Ok(r.decode()))
        else {
            continue;
        };
        let Ok(tile) = tile else { continue };

        let (w, h) = (tile.width(), tile.height());
        let edge = w.min(h);
        let square = tile
            .crop_imm((w - edge) / 2, (h - edge) / 2, edge, edge)
            .resize_exact(half, half, FilterType::Lanczos3)
            .to_rgb8();

        let (x, y) = ((index as u32 % 2) * half, (index as u32 / 2) * half);
        image::imageops::replace(&mut canvas, &square, x as i64, y as i64);
        placed += 1;
    }

    if placed == 0 {
        return Err(anyhow!("no usable images in mosaic"));
    }

    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY).encode_image(&canvas)?;
    Ok(out)
}

pub fn render_tile_from_bytes(bytes: &[u8], size: u32) -> Result<Vec<u8>> {
    let image = ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()?
        .decode()?;

    let (w, h) = (image.width(), image.height());
    let edge = w.min(h);
    let cropped = image.crop_imm((w - edge) / 2, (h - edge) / 2, edge, edge);

    // Never upscale: a 300px cover blown up to 540 looks worse than the browser
    // stretching it, and costs us the encode.
    let target = size.min(edge).max(1);
    let resized = cropped.resize_exact(target, target, FilterType::Lanczos3);

    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY)
        .encode_image(&resized.to_rgb8())?;
    Ok(out)
}

/// Re-encode an arbitrary image into a bounded JPEG suitable for storing as
/// album art — both as `cover.jpg` and embedded in every track.
///
/// Unlike `render_tile` this preserves aspect ratio: a cover is the artwork
/// itself, not a grid cell, and cropping someone's sleeve to a square on the
/// way *into* their files would be destructive.
pub fn encode_cover(bytes: &[u8], max_edge: u32, quality: u8) -> Result<Vec<u8>> {
    let image = ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()?
        .decode()?;

    let (w, h) = (image.width(), image.height());
    let scaled = if w.max(h) > max_edge {
        image.resize(max_edge, max_edge, FilterType::Lanczos3)
    } else {
        image
    };

    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, quality).encode_image(&scaled.to_rgb8())?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, RgbImage};

    fn jpeg_of(w: u32, h: u32) -> Vec<u8> {
        let img = DynamicImage::ImageRgb8(RgbImage::from_pixel(w, h, image::Rgb([200, 160, 40])));
        let mut bytes = Vec::new();
        JpegEncoder::new_with_quality(&mut bytes, 90)
            .encode_image(&img.to_rgb8())
            .unwrap();
        bytes
    }

    fn dimensions(bytes: &[u8]) -> (u32, u32) {
        let img = ImageReader::new(std::io::Cursor::new(bytes))
            .with_guessed_format()
            .unwrap()
            .decode()
            .unwrap();
        (img.width(), img.height())
    }

    #[test]
    fn downscales_a_large_cover_to_tile_size() {
        // 1200x1200 is the largest size present in the collection.
        let out = render_tile_from_bytes(&jpeg_of(1200, 1200), 360).unwrap();
        assert_eq!(dimensions(&out), (360, 360));
        assert!(
            out.len() < 60_000,
            "tile render should be far smaller than the source, got {}",
            out.len()
        );
    }

    #[test]
    fn crops_non_square_art_to_a_square() {
        let out = render_tile_from_bytes(&jpeg_of(1000, 600), 360).unwrap();
        let (w, h) = dimensions(&out);
        assert_eq!(w, h, "tiles must be square or the grid rhythm breaks");
    }

    #[test]
    fn never_upscales_small_art() {
        // A 200px cover asked for at 540 stays 200.
        let out = render_tile_from_bytes(&jpeg_of(200, 200), 540).unwrap();
        assert_eq!(dimensions(&out), (200, 200));
    }

    #[test]
    fn garbage_bytes_are_an_error_not_a_panic() {
        assert!(render_tile_from_bytes(b"this is not an image", 360).is_err());
    }

    #[test]
    fn no_art_source_is_an_error() {
        assert!(render_tile(&ArtSource::None, 360).is_err());
    }

    #[test]
    fn requested_sizes_snap_to_the_allowed_set() {
        assert_eq!(clamp_size(360), 360);
        assert_eq!(clamp_size(400), 360);
        assert_eq!(clamp_size(1), 180);
        // A hostile request can't make us allocate something enormous.
        assert_eq!(clamp_size(100_000), 540);
    }

    #[test]
    fn encode_cover_bounds_the_long_edge_without_cropping() {
        // A non-square sleeve must keep its proportions on the way into the
        // files — cropping the user's artwork would be destructive.
        let out = encode_cover(&jpeg_of(3000, 2000), 1000, 92).unwrap();
        let (w, h) = dimensions(&out);
        assert_eq!(w, 1000);
        assert_eq!(h, 667, "aspect ratio should be preserved");
    }

    #[test]
    fn encode_cover_leaves_small_art_alone() {
        let out = encode_cover(&jpeg_of(600, 600), 1000, 92).unwrap();
        assert_eq!(dimensions(&out), (600, 600));
    }

    #[test]
    fn encode_cover_rejects_non_images() {
        assert!(encode_cover(b"not an image", 1000, 92).is_err());
    }

    #[test]
    fn mosaic_composites_multiple_sources() {
        let dir = std::env::temp_dir().join("hark-mosaic-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut parts = Vec::new();
        for i in 0..3 {
            let p = dir.join(format!("{i}.jpg"));
            std::fs::write(&p, jpeg_of(500, 500)).unwrap();
            parts.push(ArtSource::File(p));
        }

        let out = render_tile(&ArtSource::Mosaic(parts), 360).unwrap();
        assert_eq!(dimensions(&out), (360, 360));
    }

    #[test]
    fn a_mosaic_with_no_readable_sources_is_an_error() {
        let parts = vec![ArtSource::File("/nonexistent/a.jpg".into())];
        assert!(render_tile(&ArtSource::Mosaic(parts), 360).is_err());
    }

    #[test]
    fn cache_round_trips_and_invalidates() {
        let cache = ArtCache::default();
        assert!(cache.get(7, 360).is_none());

        cache.put(7, 360, vec![1, 2, 3]);
        cache.put(7, 180, vec![4]);
        cache.put(8, 360, vec![5]);
        assert_eq!(cache.get(7, 360), Some(vec![1, 2, 3]));

        cache.invalidate(7);
        assert!(cache.get(7, 360).is_none());
        assert!(cache.get(7, 180).is_none());
        assert_eq!(cache.get(8, 360), Some(vec![5]), "other albums untouched");
    }
}
