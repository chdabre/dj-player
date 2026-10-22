//! What every surface and pipeline shares: a surface's [`PlaneSet`] (view uniform, rectangle
//! instances and text by plane) and the instanced quad pipeline all four pipelines are built
//! with.

use std::borrow::Cow;

use super::rects::RectBatch;
use super::scene::LAYERS;
use super::text::{Fonts, LayerText};

/// What one surface's scene needs on the GPU, for each of its planes: its view uniform,
/// rectangle instances and text.
pub(super) struct PlaneSet {
    pub(super) view_buffer: wgpu::Buffer,
    pub(super) view_bind: wgpu::BindGroup,
    pub(super) rects: RectBatch,
    /// Text by plane.
    pub(super) text: [LayerText; LAYERS],
}

impl PlaneSet {
    pub(super) fn new(
        device: &wgpu::Device,
        fonts: &mut Fonts,
        view_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let view_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("view"),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let view_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("view"),
            layout: view_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: view_buffer.as_entire_binding(),
            }],
        });
        Self {
            view_buffer,
            view_bind,
            rects: RectBatch::new(device),
            text: [LayerText::new(device, fonts), LayerText::new(device, fonts)],
        }
    }
}

/// An instanced quad pipeline: a triangle strip of four vertices per instance, drawing into
/// `format` with `blend`.
pub(super) fn quad_pipeline(
    device: &wgpu::Device,
    label: &str,
    source: &str,
    groups: &[&wgpu::BindGroupLayout],
    instance: Option<wgpu::VertexBufferLayout>,
    format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(source)),
    });
    let groups: Vec<Option<&wgpu::BindGroupLayout>> = groups.iter().copied().map(Some).collect();
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        bind_group_layouts: &groups,
        ..Default::default()
    });
    let buffers: Vec<Option<wgpu::VertexBufferLayout>> = instance.into_iter().map(Some).collect();
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs"),
            buffers: &buffers,
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs"),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleStrip,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        cache: None,
        multiview_mask: None,
    })
}
