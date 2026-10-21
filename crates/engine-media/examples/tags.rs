//! Print the tags the browser reads: `cargo run -p engine-media --example tags -- <files…>`.

fn main() {
    for path in std::env::args().skip(1) {
        match engine_media::tags(&path) {
            Ok(mut t) => {
                let art = t.artwork.take().map(|a| (a.media_type, a.data.len()));
                println!("{path}\n  {t:?}\n  artwork: {art:?}");
            }
            Err(e) => println!("{path}\n  error: {e}"),
        }
    }
}
