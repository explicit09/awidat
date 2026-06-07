//! Single-layer transform compositor.
//!
//! The first concrete implementation of the [`crate::Compositor`] trait.
//! Takes one RGBA8 input layer + a translate / scale / rotate
//! [`TransformSpec`] and renders it onto a fresh output texture
//! sized to the spec's `output_width × output_height`.
//!
//! Phase-1 scope is intentionally narrow:
//!
//! * Single layer only — extra layers in `spec.layers` are ignored
//!   (with a TODO to wire them up alongside the multi-layer blender).
//! * No blur, no opacity envelope, no particles.
//! * No mipmap chain on the layer — Phase-1 favours simplicity over
//!   filtering quality.
//!
//! Construction (`new`) requests a `wgpu::Adapter` and `wgpu::Device`,
//! which can fail on sandboxed CI runners with no GPU driver. Callers
//! should degrade gracefully if [`CompositorError::NoAdapter`] is
//! returned rather than aborting the whole render.

use bytemuck::{Pod, Zeroable};

use crate::compositor::{Compositor, CompositorError, CompositorFrame, CompositorFrameSpec};

const TRANSFORM_WGSL: &str = include_str!("shaders/transform.wgsl");

/// Buffer-row stride alignment required by `copy_texture_to_buffer`.
const COPY_BYTES_PER_ROW_ALIGNMENT: u32 = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct TransformUniforms {
    /// Column-major 4x4 matrix. WGSL `mat4x4<f32>` reads columns from
    /// memory in order, so the leftmost `[f32; 4]` here is column 0.
    transform: [[f32; 4]; 4],
}

/// One-layer transform-only [`Compositor`].
pub struct TransformCompositor {
    device: wgpu::Device,
    queue: wgpu::Queue,
    sampler: wgpu::Sampler,
    pipeline: wgpu::RenderPipeline,
    bind_layout: wgpu::BindGroupLayout,
}

impl TransformCompositor {
    /// Build the compositor. Async work (adapter + device request) is
    /// wrapped in `pollster::block_on` so callers don't need an
    /// executor.
    pub fn new() -> Result<Self, CompositorError> {
        pollster::block_on(Self::new_async())
    }

    async fn new_async() -> Result<Self, CompositorError> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::default());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await
            .ok_or_else(|| CompositorError::NoAdapter("no compatible GPU adapter".into()))?;

        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("montage-render-gpu-transform-compositor"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                },
                None,
            )
            .await
            .map_err(|e| CompositorError::NoDevice(e.to_string()))?;

        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("transform_compositor_bind_layout"),
            entries: &[
                // t_layer
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
                // sampler
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                // uniforms
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("transform_compositor_pipeline_layout"),
            bind_group_layouts: &[&bind_layout],
            push_constant_ranges: &[],
        });

        let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("transform_compositor_shader"),
            source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(TRANSFORM_WGSL)),
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("transform_compositor_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader_module,
                entry_point: "vs_main",
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader_module,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                // No culling — the rotation matrix can flip winding.
                cull_mode: None,
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("transform_compositor_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        Ok(Self {
            device,
            queue,
            sampler,
            pipeline,
            bind_layout,
        })
    }

    /// Internal helper that owns the full compose path.
    fn compose_sync(&self, spec: &CompositorFrameSpec) -> Result<CompositorFrame, CompositorError> {
        if spec.output_width == 0 || spec.output_height == 0 {
            return Err(CompositorError::BadOutputSize {
                width: spec.output_width,
                height: spec.output_height,
            });
        }
        let layer = spec.layers.first().ok_or(CompositorError::NoLayers)?;
        let expected = (layer.width as usize) * (layer.height as usize) * 4;
        if layer.rgba.len() != expected {
            return Err(CompositorError::BadLayerSize {
                expected,
                actual: layer.rgba.len(),
                width: layer.width,
                height: layer.height,
            });
        }
        // TODO(overnight-wave3-gpu-motion): honour additional layers,
        // blend modes, blur amount, and opacity envelopes.
        pollster::block_on(self.compose_async(spec, layer))
    }

    async fn compose_async(
        &self,
        spec: &CompositorFrameSpec<'_>,
        layer: &crate::compositor::LayerSpec<'_>,
    ) -> Result<CompositorFrame, CompositorError> {
        let layer_size = wgpu::Extent3d {
            width: layer.width,
            height: layer.height,
            depth_or_array_layers: 1,
        };
        let layer_tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("transform_compositor_layer"),
            size: layer_size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &layer_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            layer.rgba,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(layer.width * 4),
                rows_per_image: Some(layer.height),
            },
            layer_size,
        );
        let layer_view = layer_tex.create_view(&wgpu::TextureViewDescriptor::default());

        let matrix = layer.transform.to_matrix_column_major(
            [layer.width, layer.height],
            [spec.output_width, spec.output_height],
        );
        let uniforms = TransformUniforms { transform: matrix };
        let uniform_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("transform_compositor_uniforms"),
            size: std::mem::size_of::<TransformUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue
            .write_buffer(&uniform_buf, 0, bytemuck::bytes_of(&uniforms));

        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("transform_compositor_bind_group"),
            layout: &self.bind_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&layer_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: uniform_buf.as_entire_binding(),
                },
            ],
        });

        let output_size = wgpu::Extent3d {
            width: spec.output_width,
            height: spec.output_height,
            depth_or_array_layers: 1,
        };
        let output_tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("transform_compositor_output"),
            size: output_size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let output_view = output_tex.create_view(&wgpu::TextureViewDescriptor::default());

        let padded_bytes_per_row = padded_bytes_per_row(spec.output_width);
        let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("transform_compositor_readback"),
            size: (padded_bytes_per_row as u64) * (spec.output_height as u64),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("transform_compositor_encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("transform_compositor_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &output_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            // Two triangles, six vertices, covering the layer quad.
            pass.draw(0..6, 0..1);
        }
        encoder.copy_texture_to_buffer(
            wgpu::ImageCopyTexture {
                texture: &output_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::ImageCopyBuffer {
                buffer: &staging,
                layout: wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: Some(spec.output_height),
                },
            },
            output_size,
        );
        self.queue.submit(std::iter::once(encoder.finish()));

        let slice = staging.slice(..);
        let (sender, receiver) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        self.device.poll(wgpu::Maintain::Wait);
        receiver
            .recv()
            .map_err(|e| CompositorError::NoDevice(format!("readback channel: {e}")))?
            .map_err(|e| CompositorError::NoDevice(format!("readback map: {e:?}")))?;

        let view = slice.get_mapped_range();
        let row_bytes = (spec.output_width * 4) as usize;
        let mut rgba = Vec::with_capacity(row_bytes * spec.output_height as usize);
        for row in 0..(spec.output_height as usize) {
            let start = row * (padded_bytes_per_row as usize);
            rgba.extend_from_slice(&view[start..start + row_bytes]);
        }
        drop(view);
        staging.unmap();

        Ok(CompositorFrame {
            texture: output_tex,
            width: spec.output_width,
            height: spec.output_height,
            rgba,
        })
    }
}

impl Compositor for TransformCompositor {
    fn compose(&self, spec: &CompositorFrameSpec) -> Result<CompositorFrame, CompositorError> {
        self.compose_sync(spec)
    }
}

fn padded_bytes_per_row(width: u32) -> u32 {
    let row_bytes = width * 4;
    let align = COPY_BYTES_PER_ROW_ALIGNMENT;
    row_bytes.div_ceil(align) * align
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn padded_row_alignment_is_correct() {
        // Same contract as the transitions pipeline — wgpu requires
        // every row of a texture-to-buffer copy to start on a 256-byte
        // boundary.
        assert_eq!(padded_bytes_per_row(64), 256);
        assert_eq!(padded_bytes_per_row(63), 256);
        assert_eq!(padded_bytes_per_row(65), 512);
    }

    #[test]
    fn rejects_zero_output_dimensions() {
        // No GPU required for this check — `compose_sync` validates
        // dimensions before talking to wgpu.
        let Ok(compositor) = TransformCompositor::new() else {
            // No adapter; skip gracefully.
            return;
        };
        let layer = vec![0u8; 4];
        let spec = CompositorFrameSpec {
            output_width: 0,
            output_height: 1,
            layers: vec![crate::compositor::LayerSpec {
                width: 1,
                height: 1,
                rgba: &layer,
                transform: crate::compositor::TransformSpec::default(),
                blend: crate::compositor::BlendMode::Normal,
            }],
        };
        match compositor.compose(&spec) {
            Err(err) => assert!(matches!(err, CompositorError::BadOutputSize { .. })),
            Ok(_) => panic!("expected BadOutputSize error"),
        }
    }
}
