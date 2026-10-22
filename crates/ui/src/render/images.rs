//! Pictures (artwork thumbnails): one instance per picture, its texture in group 1, with the
//! rect shader's rounded-corner coverage. Textures are made the first frame a picture shows
//! and dropped after a while unused.

use std::collections::HashMap;
use std::ops::Range;

use super::plane::quad_pipeline;
use super::scene::{Image, LAYERS, Scene};

const SHADER: &str = r#"
struct View { xform: vec4<f32>, size: vec4<f32> };
@group(0) @binding(0) var<uniform> view: View;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var smp: sampler;

struct Inst {
    @location(0) rect: vec4<f32>,
    @location(1) params: vec4<f32>,   // radius, opacity
};
struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) half: vec2<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) params: vec2<f32>,
};

@vertex
fn vs(@builtin(vertex_index) vi: u32, i: Inst) -> Out {
    let corner = vec2<f32>(f32(vi & 1u), f32(vi >> 1u));
    let s = view.xform.x;
    let p0 = i.rect.xy * s + view.xform.yz;
    let size = i.rect.zw * s;
    let p = p0 + corner * size;
    var o: Out;
    o.pos = vec4(p.x / view.size.x * 2.0 - 1.0, 1.0 - p.y / view.size.y * 2.0, 0.0, 1.0);
    o.local = p - (p0 + size * 0.5);
    o.half = size * 0.5;
    o.uv = corner;
    o.params = vec2(i.params.x * s, i.params.y);
    return o;
}

fn sd_round_box(p: vec2<f32>, half: vec2<f32>, r: f32) -> f32 {
    let q = abs(p) - half + vec2(r);
    return length(max(q, vec2(0.0))) + min(max(q.x, q.y), 0.0) - r;
}

@fragment
fn fs(o: Out) -> @location(0) vec4<f32> {
    let r = min(o.params.x, min(o.half.x, o.half.y));
    let coverage = clamp(0.5 - sd_round_box(o.local, o.half, r), 0.0, 1.0);
    let c = textureSample(tex, smp, o.uv);
    let a = c.a * o.params.y * coverage;
    return vec4(c.rgb * a, a);
}
"#;

const ATTRS: [wgpu::VertexAttribute; 2] = wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4];
const INSTANCE_BYTES: u64 = 8 * 4;
/// Pictures per frame (the browse list shows seven).
const MAX_IMAGES: usize = 64;
/// A picture's texture is dropped after this many frames unused.
const KEEP_FRAMES: u64 = 600;

/// A picture on the GPU.
struct Texture {
    bind: wgpu::BindGroup,
    used: u64,
}

pub(super) struct Images {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    instances: wgpu::Buffer,
    textures: HashMap<u64, Texture>,
    frame_no: u64,
}

impl Images {
    pub(super) fn new(
        device: &wgpu::Device,
        view_layout: &wgpu::BindGroupLayout,
        format: wgpu::TextureFormat,
    ) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("image"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline = quad_pipeline(
            device,
            "image",
            SHADER,
            &[view_layout, &layout],
            Some(wgpu::VertexBufferLayout {
                array_stride: INSTANCE_BYTES,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &ATTRS,
            }),
            format,
            Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
        );
        let instances = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("image instances"),
            size: MAX_IMAGES as u64 * INSTANCE_BYTES,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            pipeline,
            layout,
            instances,
            textures: HashMap::new(),
            frame_no: 0,
        }
    }

    /// Make textures for the scene's pictures not on the GPU yet, drop those unused for a
    /// while, and write the instances, plane after plane; returns each plane's instances.
    pub(super) fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        sampler: &wgpu::Sampler,
        srgb: bool,
        scene: &Scene,
    ) -> [Range<u32>; LAYERS] {
        self.frame_no += 1;
        let now = self.frame_no;
        let mut planes: [Range<u32>; LAYERS] = Default::default();
        let mut start = 0;
        for (plane, range) in scene.planes.iter().zip(&mut planes) {
            let end = (start + plane.images.len() as u32).min(MAX_IMAGES as u32);
            *range = start..end;
            start = end;
        }
        let n = start as usize;
        let mut bytes: Vec<u8> = Vec::with_capacity(n * INSTANCE_BYTES as usize);
        for img in pictures(scene).take(n) {
            let pic = &img.picture;
            if let Some(t) = self.textures.get_mut(&pic.id) {
                t.used = now;
            } else if pic.width > 0 && pic.rgba.len() == (pic.width * pic.height * 4) as usize {
                let format = if srgb {
                    wgpu::TextureFormat::Rgba8UnormSrgb
                } else {
                    wgpu::TextureFormat::Rgba8Unorm
                };
                let size = wgpu::Extent3d {
                    width: pic.width,
                    height: pic.height,
                    depth_or_array_layers: 1,
                };
                let texture = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("picture"),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    &pic.rgba,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(pic.width * 4),
                        rows_per_image: Some(pic.height),
                    },
                    size,
                );
                let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("picture"),
                    layout: &self.layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(sampler),
                        },
                    ],
                });
                self.textures.insert(pic.id, Texture { bind, used: now });
            }
            for v in [img.x, img.y, img.w, img.h, img.radius, 1.0, 0.0, 0.0] {
                bytes.extend_from_slice(&v.to_ne_bytes());
            }
        }
        self.textures
            .retain(|_, t| now.saturating_sub(t.used) <= KEEP_FRAMES);
        if n > 0 {
            queue.write_buffer(&self.instances, 0, &bytes);
        }
        planes
    }

    /// Draw `scene`'s pictures `range` (as uploaded).
    pub(super) fn draw(
        &self,
        pass: &mut wgpu::RenderPass,
        view_bind: &wgpu::BindGroup,
        scene: &Scene,
        range: Range<u32>,
    ) {
        if range.is_empty() {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, view_bind, &[]);
        pass.set_vertex_buffer(0, self.instances.slice(..));
        let (skip, take) = (range.start as usize, range.len());
        for (k, img) in pictures(scene).enumerate().skip(skip).take(take) {
            if let Some(t) = self.textures.get(&img.picture.id) {
                pass.set_bind_group(1, &t.bind, &[]);
                pass.draw(0..4, k as u32..k as u32 + 1);
            }
        }
    }
}

/// Every picture of the scene, plane after plane.
fn pictures(scene: &Scene) -> impl Iterator<Item = &Image> {
    scene.planes.iter().flat_map(|p| &p.images)
}
