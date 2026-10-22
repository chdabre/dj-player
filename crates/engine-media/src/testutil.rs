//! Fixtures for the tests: WAV files and ID3v2 tags, generated on the fly.

use std::path::{Path, PathBuf};

pub fn write_wav(path: &Path, rate: u32, channels: u16, samples: &[f32]) {
    let spec = hound::WavSpec {
        channels,
        sample_rate: rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for &s in samples {
        w.write_sample(s).unwrap();
    }
    w.finalize().unwrap();
}

pub fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("engine-media-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

pub fn sine(rate: u32, hz: f32, frames: usize) -> Vec<f32> {
    let w = std::f32::consts::TAU * hz / rate as f32;
    (0..frames).map(|i| 0.5 * (w * i as f32).sin()).collect()
}

/// An ID3v2.3 tag of `size` bytes whose payload is full of fake MPEG frame syncs, like
/// embedded artwork can be.
pub fn id3_tag(size: usize) -> Vec<u8> {
    let mut tag = b"ID3\x03\x00\x00".to_vec();
    tag.extend((0..4).rev().map(|k| ((size >> (7 * k)) & 0x7f) as u8));
    tag.extend((0..size).map(|i| {
        if i % 417 == 0 {
            0xff
        } else if i % 417 == 1 {
            0xfb
        } else {
            0x90
        }
    }));
    tag
}

/// One ID3v2.3 frame: id, plain (not synchsafe) size, no flags.
fn id3_frame(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut f = id.to_vec();
    f.extend((body.len() as u32).to_be_bytes());
    f.extend([0, 0]);
    f.extend(body);
    f
}

/// An ID3v2.3 tag holding `frames`.
pub fn id3_with(frames: &[Vec<u8>]) -> Vec<u8> {
    let body: Vec<u8> = frames.concat();
    let mut tag = b"ID3\x03\x00\x00".to_vec();
    tag.extend((0..4).rev().map(|k| ((body.len() >> (7 * k)) & 0x7f) as u8));
    tag.extend(body);
    tag
}

pub fn text_frame(id: &[u8; 4], text: &str) -> Vec<u8> {
    id3_frame(id, &[&[0u8][..], text.as_bytes()].concat())
}

/// An attached picture of `kind` (0 other, 3 front cover).
pub fn picture_frame(kind: u8, data: &[u8]) -> Vec<u8> {
    id3_frame(
        b"APIC",
        &[b"\x00image/png\x00", &[kind, 0][..], data].concat(),
    )
}

/// A short stereo WAV at 44.1 kHz behind `tag`, written to `name`: half a second long.
pub fn tagged_wav(name: &str, tag: &[u8]) -> PathBuf {
    let wav = tmp(&format!("{name}-src.wav"));
    write_wav(&wav, 44_100, 2, &[0.25; 2 * 22_050]);
    let mut bytes = tag.to_vec();
    bytes.extend(std::fs::read(&wav).unwrap());
    let path = tmp(name);
    std::fs::write(&path, bytes).unwrap();
    path
}
