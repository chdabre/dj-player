//! Offline render bench: ns per output frame for one deck through `Engine::process`, at the
//! block size the Pi will run (128 frames). Run it on the Mac and on a Pi 5 and compare the
//! per-block time against the 1.3 ms half-budget from docs/research/engine.md. Each iteration
//! starts with a seek, so the keylock case includes one stretcher prime per second.

use std::hint::black_box;
use std::sync::Arc;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use engine_core::render::index_ramp;
use engine_core::{DeckCommand, Engine, EngineConfig, Source};

const SR: u32 = 48_000;
const BLOCK: usize = 128;
/// One second of output per iteration.
const FRAMES: usize = SR as usize;

fn bench_render(c: &mut Criterion) {
    let source = Source::from_interleaved(SR, &index_ramp(60 * SR as usize, 1.0, 440.0, SR));
    let mut group = c.benchmark_group("offline_render");
    group.throughput(Throughput::Elements(FRAMES as u64));

    for (name, rate, keylock) in [
        ("rate_1.0", 1.0, false),
        ("rate_1.08", 1.08, false),
        ("reverse", -1.0, false),
        ("keylock_1.08", 1.08, true),
    ] {
        let (mut engine, mut handle) = Engine::new(EngineConfig {
            sample_rate: SR,
            channels: 2,
            decks: 1,
        });
        handle
            .send_deck(0, DeckCommand::Load(Arc::clone(&source)))
            .unwrap();
        handle
            .send_deck(0, DeckCommand::SetKeylock(keylock))
            .unwrap();
        handle.send_deck(0, DeckCommand::Play).unwrap();
        let mut out = vec![0.0f32; BLOCK * 2];
        engine.process(&mut out, BLOCK);
        handle.collect_retired();

        group.bench_function(name, |b| {
            b.iter(|| {
                handle
                    .send_deck(0, DeckCommand::Seek(30.0 * SR as f64))
                    .unwrap();
                handle.send_deck(0, DeckCommand::SetRate(rate)).unwrap();
                for _ in 0..FRAMES / BLOCK {
                    engine.process(black_box(&mut out), BLOCK);
                }
                black_box(&out);
            })
        });
    }
    group.finish();
}

criterion_group!(benches, bench_render);
criterion_main!(benches);
