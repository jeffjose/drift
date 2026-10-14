//! Frames survive the movie writer and reader: what's decoded is the
//! picture that was encoded, at the time it was encoded for, with its
//! colours where they were (the right matrix and range both ways).

use glide::media::FrameSource;
use glide::media::writer::{MovieWriter, WriterSpec, fill_bgra};
use glide::project::model::Codec;

const W: u32 = 640;
const H: u32 = 400;
const FPS: u32 = 30;

/// Mean absolute difference per channel, and the share of channel values
/// more than `tolerance` levels off.
fn difference(a: &[u8], b: &[u8], stride: usize, tolerance: u8) -> (f64, f64) {
    let (mut sum, mut off, mut n) = (0u64, 0u64, 0u64);
    for y in 0..H as usize {
        for x in 0..W as usize * 4 {
            // Alpha is not carried.
            if x % 4 == 3 {
                continue;
            }
            let d = a[y * W as usize * 4 + x].abs_diff(b[y * stride + x]);
            sum += d as u64;
            off += u64::from(d > tolerance);
            n += 1;
        }
    }
    (sum as f64 / n as f64, off as f64 / n as f64)
}

fn round_trip(codec: Codec, name: &str) {
    let dir = std::env::temp_dir().join(format!("glide-media-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("movie.mov");
    let mut writer = MovieWriter::create(&path, &WriterSpec { width: W, height: H, fps: FPS, codec, bitrate: 8_000_000, quality: Some(0.95), audio: false }).unwrap();
    let frames = 45;
    for i in 0..frames {
        let px = glide::synth::screen_bgra(W, H, i as f64 * 1000.0 / FPS as f64);
        let pb = writer.buffer().unwrap();
        fill_bgra(&pb, &px, W, H, W as usize * 4);
        writer.push_frame(&pb).unwrap();
    }
    writer.finish().unwrap();

    let mut source = FrameSource::open(&path).unwrap();
    assert_eq!(source.size(), (W, H));
    assert!((source.duration_ms() - frames as f64 * 1000.0 / FPS as f64).abs() < 40.0, "duration {}", source.duration_ms());
    // Out of order, so seeking is exercised as well as reading on.
    for i in [0, 1, 20, 44, 7] {
        let t = i as f64 * 1000.0 / FPS as f64;
        let want = glide::synth::screen_bgra(W, H, t);
        let frame = source.frame_at(t).unwrap_or_else(|| panic!("no frame at {t} ms"));
        assert!((frame.pts_ms - t).abs() < 1.0, "frame for {t} ms is stamped {} ms", frame.pts_ms);
        let (mean, off) = frame.with_bgra(|v| difference(&want, v.bgra, v.stride, 12));
        // Flat interface colours: a wrong matrix or range shifts every
        // pixel by several levels; compression and chroma subsampling only
        // touch edges.
        assert!(mean < 1.5, "{name} frame {i}: mean error {mean:.2} levels");
        assert!(off < 0.02, "{name} frame {i}: {:.2}% of values are far off", off * 100.0);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn h264_round_trip_keeps_the_picture() {
    round_trip(Codec::H264, "h264");
}

#[test]
fn hevc_round_trip_keeps_the_picture() {
    round_trip(Codec::Hevc, "hevc");
}
