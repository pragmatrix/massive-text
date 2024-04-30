use std::{
    mem::{self, size_of, size_of_val},
    result,
};

use log::info;
use massive_geometry::Matrix4;
use wgpu::{util::DeviceExt, Device, StoreOp};

use crate::{
    pipelines, pods,
    primitives::{Pipeline, Primitive},
    shape,
    text_layer::{self, TextLayer},
    texture::{self, Texture},
};

pub struct Renderer<'window> {
    surface: wgpu::Surface<'window>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub surface_config: wgpu::SurfaceConfiguration,

    view_projection_buffer: wgpu::Buffer,
    view_projection_bind_group: wgpu::BindGroup,

    // TODO: this doesn't belong here and is used only for specific pipelines. We need some
    // per-pipeline information types.
    pub texture_bind_group_layout: texture::BindGroupLayout,
    pub text_layer_bind_group_layout: text_layer::BindGroupLayout,

    pipelines: Vec<(Pipeline, wgpu::RenderPipeline)>,

    index_buffer: QuadIndexBuffer,
}

impl<'window> Renderer<'window> {
    /// Creates a new renderer and reconfigures the surface according to the given configuration.
    pub fn new(
        device: wgpu::Device,
        queue: wgpu::Queue,
        surface: wgpu::Surface<'window>,
        surface_config: wgpu::SurfaceConfiguration,
    ) -> Self {
        let view_projection_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("View Projection Matrix Buffer"),
            size: mem::size_of::<pods::Matrix4>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let (view_projection_bind_group_layout, view_projection_bind_group) =
            pipelines::create_view_projection_bind_group(&device, &view_projection_buffer);

        let texture_bind_group_layout = texture::BindGroupLayout::new(&device);

        let text_layer_bind_group_layout = text_layer::BindGroupLayout::new(&device);

        let shape_bind_group_layout = shape::BindGroupLayout::new(&device);

        let index_buffer = QuadIndexBuffer::new(&device);

        let pipelines = {
            let targets = [Some(wgpu::ColorTargetState {
                format: surface_config.format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })];

            pipelines::create(
                &device,
                &view_projection_bind_group_layout,
                &texture_bind_group_layout,
                &text_layer_bind_group_layout,
                &shape_bind_group_layout,
                &targets,
            )
        };

        let mut renderer = Self {
            device,
            queue,
            surface,
            surface_config,
            view_projection_buffer,
            view_projection_bind_group,
            texture_bind_group_layout,
            text_layer_bind_group_layout,
            pipelines,

            index_buffer,
        };

        renderer.reconfigure_surface();
        renderer
    }

    // TODO: Can't we handle SurfaceError::Lost here by just reconfiguring the surface and trying
    // again?
    #[tracing::instrument(skip_all)]
    pub fn render_and_present(
        &mut self,
        view_projection_matrix: &Matrix4,
        primitives: &[Primitive],
    ) -> result::Result<(), wgpu::SurfaceError> {
        let surface_texture = self.surface.get_current_texture()?;
        let surface_view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        // Prepare the index buffer.

        self.index_buffer.ensure_quads(
            &self.device,
            primitives
                .iter()
                .map(|p| p.quads())
                .max()
                .unwrap_or_default(),
        );

        self.queue_view_projection_matrix(view_projection_matrix);

        let command_buffer = {
            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("Render Encoder"),
                });

            {
                let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Render Pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &surface_view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                            store: StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });

                for pipeline in &self.pipelines {
                    let kind = pipeline.0;
                    let pipeline = &pipeline.1;
                    render_pass.set_pipeline(pipeline);
                    render_pass.set_bind_group(0, &self.view_projection_bind_group, &[]);
                    render_pass.set_index_buffer(
                        self.index_buffer.buffer.slice(..),
                        wgpu::IndexFormat::Uint16,
                    );

                    for primitive in primitives.iter().filter(|p| p.pipeline() == kind) {
                        match primitive {
                            Primitive::Texture(Texture {
                                bind_group,
                                vertex_buffer,
                                ..
                            }) => {
                                render_pass.set_bind_group(1, bind_group, &[]);
                                render_pass.set_vertex_buffer(0, vertex_buffer.slice(..));
                                render_pass.draw_indexed(
                                    0..Self::QUAD_INDICES.len() as u32,
                                    0,
                                    0..1,
                                );
                            }
                            Primitive::TextLayer(TextLayer {
                                fragment_shader_bind_group,
                                model_matrix,
                                vertex_buffer,
                                instance_buffer,
                                instance_count,
                            }) => {
                                let text_layer_matrix = *view_projection_matrix * model_matrix;

                                // OO: Set bind group only once and update the buffer?
                                self.queue_view_projection_matrix(&text_layer_matrix);
                                render_pass.set_bind_group(
                                    0,
                                    &self.view_projection_bind_group,
                                    &[],
                                );

                                render_pass.set_bind_group(1, fragment_shader_bind_group, &[]);
                                render_pass.set_vertex_buffer(0, vertex_buffer.slice(..));
                                render_pass.set_vertex_buffer(1, instance_buffer.slice(..));

                                render_pass.draw_indexed(
                                    0..Self::QUAD_INDICES.len() as u32,
                                    0,
                                    0..*instance_count as u32,
                                )
                            }
                        }
                    }
                }
            }
            encoder.finish()
        };

        self.queue.submit([command_buffer]);
        surface_texture.present();
        Ok(())
    }

    const QUAD_INDICES: &'static [u16] = &[0, 1, 2, 0, 2, 3];

    fn queue_view_projection_matrix(&self, view_projection_matrix: &Matrix4) {
        let view_projection_uniform = {
            let m: cgmath::Matrix4<f32> = view_projection_matrix
                .cast()
                .expect("matrix casting to f32 failed");
            pods::Matrix4(m.into())
        };

        self.queue.write_buffer(
            &self.view_projection_buffer,
            0,
            bytemuck::cast_slice(&[view_projection_uniform]),
        )
    }

    // A Matrix that projects from normalized view coordinates -1.0 to 1.0 (3D, all axis, Z from 0.1
    // to 100) to 2D coordinates.

    // A Matrix that translates from the WGPU coordinate system to surface coordinates.
    pub fn surface_matrix(&self) -> Matrix4 {
        let (width, height) = self.surface_size();
        Matrix4::from_nonuniform_scale(width as f64 / 2.0, (height as f64 / 2.0) * -1.0, 1.0)
            * Matrix4::from_translation(cgmath::Vector3::new(1.0, -1.0, 0.0))
    }

    /// Resizes the surface, if necessary.
    /// Keeps the surface size at least 1x1.
    pub fn resize_surface(&mut self, new_size: (u32, u32)) {
        let new_surface_size = (new_size.0.max(1), new_size.1.max(1));

        if new_surface_size == self.surface_size() {
            return;
        }
        let config = &mut self.surface_config;
        config.width = new_surface_size.0;
        config.height = new_surface_size.1;

        self.reconfigure_surface();
    }

    /// Returns the current surface size.
    /// It may not match the window's size, for example if the window's size is 0,0.
    pub fn surface_size(&self) -> (u32, u32) {
        let config = &self.surface_config;
        (config.width, config.height)
    }

    pub fn reconfigure_surface(&mut self) {
        info!("Reconfiguring surface {:?}", self.surface_config);
        self.surface.configure(&self.device, &self.surface_config)
    }

    fn prepare_index_buffer(&mut self, max_quads: usize) {}
}

struct QuadIndexBuffer {
    buffer: wgpu::Buffer,
}

impl QuadIndexBuffer {
    pub fn new(device: &Device) -> Self {
        // OO: Provide a good initial size.
        const NO_INDICES: [u16; 0] = [];
        Self {
            buffer: Self::create_buffer(device, &NO_INDICES),
        }
    }

    pub fn quads(&self) -> usize {
        (self.buffer.size() as usize) / size_of_val(Self::QUAD_INDICES)
    }

    pub fn ensure_quads(&mut self, device: &Device, new_quad_count: usize) {
        if new_quad_count <= self.quads() {
            return;
        }

        let indices = Self::generate_array(self, new_quad_count);
        let buffer = Self::create_buffer(device, &indices);

        self.buffer = buffer;
    }

    fn generate_array(&self, quads: usize) -> Vec<u16> {
        let mut v = Vec::with_capacity(quads * Self::QUAD_INDICES.len());

        (0..quads).for_each(|quad_index| {
            v.extend(
                Self::QUAD_INDICES
                    .iter()
                    .map(|i| *i + (quad_index * 4) as u16),
            )
        });

        v
    }

    const QUAD_INDICES: &'static [u16] = &[0, 1, 2, 0, 2, 3];

    fn create_buffer(device: &Device, indices: &[u16]) -> wgpu::Buffer {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Quad Index Buffer"),
            contents: bytemuck::cast_slice(indices),
            usage: wgpu::BufferUsages::INDEX,
        })
    }
}
