//! Render targets other than the window: the device screen's offscreen texture while the
//! stand-in shell is on (copied into the shell's display slot), and screenshot readback.

use super::quad_pipeline;

/// Copies the device layer's offscreen texture into the shell's display slot.
const BLIT_SHADER: &str = r#"
struct Rect { clip: vec4<f32> };  // left, top, right, bottom in clip space
@group(0) @binding(0) var<uniform> rect: Rect;
@group(0) @binding(1) var tex: texture_2d<f32>;
@group(0) @binding(2) var smp: sampler;
struct Out { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };

@vertex
fn vs(@builtin(vertex_index) vi: u32) -> Out {
    let c = vec2<f32>(f32(vi & 1u), f32(vi >> 1u));
    var o: Out;
    o.pos = vec4(mix(rect.clip.x, rect.clip.z, c.x), mix(rect.clip.y, rect.clip.w, c.y), 0.0, 1.0);
    o.uv = c;
    return o;
}

@fragment
fn fs(o: Out) -> @location(0) vec4<f32> {
    return textureSample(tex, smp, o.uv);
}
"#;

/// The device layer's offscreen target, and the bind group that copies it.
struct Target {
    size: (u32, u32),
    view: wgpu::TextureView,
    bind: wgpu::BindGroup,
}

/// The device screen rendered offscreen and copied into a slot of the window.
pub(super) struct Offscreen {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    rect: wgpu::Buffer,
    target: Option<Target>,
}

impl Offscreen {
    pub(super) fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("blit"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline = quad_pipeline(device, "blit", BLIT_SHADER, &[&layout], None, format, None);
        let rect = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("blit rect"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            pipeline,
            layout,
            rect,
            target: None,
        }
    }

    /// The offscreen target, made (or remade) `size` physical pixels.
    pub(super) fn target(
        &mut self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        sampler: &wgpu::Sampler,
        size: (u32, u32),
    ) -> wgpu::TextureView {
        if let Some(t) = self.target.as_ref().filter(|t| t.size == size) {
            return t.view.clone();
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("device screen"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("blit"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.rect.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        });
        self.target = Some(Target {
            size,
            view: view.clone(),
            bind,
        });
        view
    }

    /// Encode the copy of the offscreen target into `target` (`w`×`h`) at physical pixel
    /// `at`, its own size.
    pub(super) fn blit(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target: &wgpu::TextureView,
        (w, h): (u32, u32),
        at: (f32, f32),
    ) -> wgpu::CommandBuffer {
        let t = self.target.as_ref().expect("blit before target");
        let (wf, hf) = (w as f32, h as f32);
        let clip = [
            at.0 / wf * 2.0 - 1.0,
            1.0 - at.1 / hf * 2.0,
            (at.0 + t.size.0 as f32) / wf * 2.0 - 1.0,
            1.0 - (at.1 + t.size.1 as f32) / hf * 2.0,
        ];
        let bytes: Vec<u8> = clip.iter().flat_map(|v| v.to_ne_bytes()).collect();
        queue.write_buffer(&self.rect, 0, &bytes);
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("device screen into the shell"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &t.bind, &[]);
            pass.draw(0..4, 0..1);
        }
        encoder.finish()
    }
}

/// Run `draws` into `texture` (`w`×`h`, `format`) and read it back as RGBA8 rows.
pub(super) fn read_back(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    format: wgpu::TextureFormat,
    (w, h): (u32, u32),
    draws: Vec<wgpu::CommandBuffer>,
) -> Vec<u8> {
    // Rows padded to the 256-byte copy alignment.
    let row = (w * 4).div_ceil(256) * 256;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("screenshot readback"),
        size: u64::from(row * h),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    let mut commands = draws;
    commands.push(encoder.finish());
    queue.submit(commands);
    readback
        .slice(..)
        .map_async(wgpu::MapMode::Read, |r| r.expect("map screenshot"));
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let padded = readback
        .slice(..)
        .get_mapped_range()
        .expect("screenshot mapping")
        .to_vec();
    let mut pixels: Vec<u8> = padded
        .chunks_exact(row as usize)
        .flat_map(|r| r[..(w * 4) as usize].iter().copied())
        .collect();
    if matches!(
        format,
        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
    ) {
        for px in pixels.as_chunks_mut::<4>().0.iter_mut() {
            px.swap(0, 2);
        }
    }
    pixels
}
