//! Text through glyphon: one font system and glyph atlas for the app, and per layer a text
//! renderer with its shaped buffers cached by content, so only changing strings (the
//! readouts) re-shape each frame.

use glyphon::cosmic_text::{Ellipsize, EllipsizeHeightLimit};
use glyphon::{
    Attrs, Buffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping, SwashCache,
    TextArea, TextAtlas, TextBounds, TextRenderer, Viewport, Wrap,
};

use super::scene::{Align, Scene, Weight};

const FONT_REGULAR: &[u8] = include_bytes!("../../assets/fonts/BarlowCondensed-Regular.ttf");
const FONT_SEMIBOLD: &[u8] = include_bytes!("../../assets/fonts/BarlowCondensed-SemiBold.ttf");
const FAMILY: &str = "Barlow Condensed";

/// What every layer's text shares: fonts, the glyph rasteriser and the atlas.
pub(super) struct Fonts {
    fonts: FontSystem,
    swash: SwashCache,
    cache: Cache,
    pub(super) atlas: TextAtlas,
}

impl Fonts {
    pub(super) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
    ) -> Self {
        let mut fonts = FontSystem::new();
        fonts.db_mut().load_font_data(FONT_REGULAR.to_vec());
        fonts.db_mut().load_font_data(FONT_SEMIBOLD.to_vec());
        let cache = Cache::new(device);
        let atlas = TextAtlas::new(device, queue, &cache, format);
        Self {
            fonts,
            swash: SwashCache::new(),
            cache,
            atlas,
        }
    }
}

/// Cached shaped text, reused while its content is unchanged.
struct Slot {
    key: (String, u32, Weight, u32, Option<u32>),
    buffer: Buffer,
    width: f32,
}

/// One layer's text: its renderer, viewport and shaped buffers (one per scene text, by index).
pub(super) struct LayerText {
    viewport: Viewport,
    renderer: TextRenderer,
    slots: Vec<Slot>,
}

impl LayerText {
    pub(super) fn new(device: &wgpu::Device, fonts: &mut Fonts) -> Self {
        Self {
            viewport: Viewport::new(device, &fonts.cache),
            renderer: TextRenderer::new(
                &mut fonts.atlas,
                device,
                wgpu::MultisampleState::default(),
                None,
            ),
            slots: Vec::new(),
        }
    }

    /// Shape (or reuse) `scene`'s texts and prepare them for a `w`×`h` target placed by
    /// `xform` (scale, offset x, offset y).
    pub(super) fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        f: &mut Fonts,
        scene: &Scene,
        (w, h): (u32, u32),
        (s, ox, oy): (f32, f32, f32),
    ) {
        self.viewport.update(
            queue,
            Resolution {
                width: w,
                height: h,
            },
        );
        for (i, t) in scene.texts.iter().enumerate() {
            let key = (
                t.text.clone(),
                t.size.to_bits(),
                t.weight,
                t.line_height.to_bits(),
                t.max_width.map(f32::to_bits),
            );
            if self.slots.get(i).is_some_and(|slot| slot.key == key) {
                continue;
            }
            let metrics = Metrics::new(t.size, t.size * t.line_height);
            let weight = match t.weight {
                Weight::Regular => glyphon::Weight::NORMAL,
                Weight::SemiBold => glyphon::Weight::SEMIBOLD,
            };
            let attrs = Attrs::new().family(Family::Name(FAMILY)).weight(weight);
            if i == self.slots.len() {
                self.slots.push(Slot {
                    key: key.clone(),
                    buffer: Buffer::new(&mut f.fonts, metrics),
                    width: 0.0,
                });
            }
            let slot = &mut self.slots[i];
            slot.buffer.set_metrics(metrics);
            slot.buffer.set_size(t.max_width, None);
            slot.buffer.set_wrap(Wrap::None);
            slot.buffer.set_ellipsize(match t.max_width {
                Some(_) => Ellipsize::End(EllipsizeHeightLimit::Lines(1)),
                None => Ellipsize::None,
            });
            slot.buffer
                .set_text(&t.text, &attrs, Shaping::Advanced, None);
            slot.buffer.shape_until_scroll(&mut f.fonts, false);
            slot.width = slot
                .buffer
                .layout_runs()
                .map(|r| r.line_w)
                .fold(0.0, f32::max);
            slot.key = key;
        }
        self.slots.truncate(scene.texts.len());
        let bounds = TextBounds {
            left: 0,
            top: 0,
            right: w as i32,
            bottom: h as i32,
        };
        let areas = scene.texts.iter().zip(&self.slots).map(|(t, slot)| {
            let left = match t.align {
                Align::Left => t.x,
                Align::Center => t.x - slot.width * 0.5,
                Align::Right => t.x - slot.width,
            };
            let [r, g, b, a] = t.color.0;
            let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            TextArea {
                buffer: &slot.buffer,
                left: ox + left * s,
                top: oy + t.y * s,
                scale: s,
                bounds,
                default_color: Color::rgba(byte(r), byte(g), byte(b), byte(a)),
                custom_glyphs: &[],
            }
        });
        if let Err(e) = self.renderer.prepare(
            device,
            queue,
            &mut f.fonts,
            &mut f.atlas,
            &self.viewport,
            areas,
            &mut f.swash,
        ) {
            eprintln!("text prepare: {e}");
        }
    }

    pub(super) fn render(&self, atlas: &TextAtlas, pass: &mut wgpu::RenderPass) {
        if let Err(e) = self.renderer.render(atlas, &self.viewport, pass) {
            eprintln!("text render: {e}");
        }
    }
}
