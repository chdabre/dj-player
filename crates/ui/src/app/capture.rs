//! Screenshots: `--screenshot` renders the frame offscreen once what it waits for is ready,
//! and writes it as a PNG.

pub(super) fn write_png(
    path: &std::path::Path,
    w: u32,
    h: u32,
    rgba: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut enc = png::Encoder::new(file, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(rgba)?;
    Ok(())
}
