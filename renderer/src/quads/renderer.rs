use std::rc::Rc;

use anyhow::Result;
use itertools::Itertools;
use massive_geometry::Matrix4;
use massive_shapes::{Quad, QuadsShape, Shape};
use wgpu::{
    util::{BufferInitDescriptor, DeviceExt},
    BufferUsages,
};

use crate::{
    pods::ColorVertex,
    renderer::{PreparationContext, RenderContext},
    tools::{create_pipeline, QuadIndexBuffer},
};

pub struct QuadsRenderer {
    pipeline: wgpu::RenderPipeline,
    index_buffer: QuadIndexBuffer,

    layers: Vec<QuadsLayer>,
}

struct QuadsLayer {
    model_matrix: Matrix4,
    vertex_buffer: wgpu::Buffer,
    quad_count: usize,
}

impl QuadsRenderer {
    pub fn new(
        device: &wgpu::Device,
        target_format: wgpu::TextureFormat,
        view_projection_bind_group_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let shader = &device.create_shader_module(wgpu::include_wgsl!("quads.wgsl"));

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Quads Pipeline Layout"),
            bind_group_layouts: &[view_projection_bind_group_layout],
            push_constant_ranges: &[],
        });

        let targets = [Some(wgpu::ColorTargetState {
            format: target_format,
            blend: Some(wgpu::BlendState::ALPHA_BLENDING),
            write_mask: wgpu::ColorWrites::ALL,
        })];

        let vertex_layout = [ColorVertex::layout()];

        let pipeline = create_pipeline(
            "Quads Pipeline",
            device,
            shader,
            "fs_quad",
            &vertex_layout,
            &pipeline_layout,
            &targets,
        );

        Self {
            pipeline,
            index_buffer: QuadIndexBuffer::new(device),
            layers: Vec::new(),
        }
    }

    pub fn prepare(&mut self, context: &mut PreparationContext, shapes: &[Shape]) -> Result<()> {
        let grouped = shapes
            .iter()
            .filter_map(|quads| match quads {
                Shape::Quads(shape) => Some(shape),
                _ => None,
            })
            .into_group_map_by(|shape| Rc::as_ptr(&shape.model_matrix));

        self.layers.clear();
        if grouped.len() > self.layers.len() {
            self.layers.reserve(grouped.len() - self.layers.len())
        }

        let mut max_quads = 0;

        for (_, shapes) in grouped {
            // NB: could deref the pointer here using unsafe.
            let matrix = &shapes[0].model_matrix;
            if let Some(quads_layer) = self.prepare_quads(context, matrix, &shapes)? {
                max_quads = max_quads.max(quads_layer.quad_count);
                self.layers.push(quads_layer)
            }
        }

        self.index_buffer
            .ensure_can_index_num_quads(context.device, max_quads);

        Ok(())
    }

    pub fn render<'rpass>(&'rpass self, context: &mut RenderContext<'_, 'rpass>) {
        let pass = &mut context.pass;
        pass.set_pipeline(&self.pipeline);
        // DI: May do this inside this renderer and pass a Matrix to prepare?.
        pass.set_bind_group(0, context.view_projection_bind_group, &[]);
        // DI: May share index buffers between renderers?
        pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint16);

        for QuadsLayer {
            model_matrix,
            vertex_buffer,
            quad_count,
        } in &self.layers
        {
            let text_layer_matrix = context.view_projection_matrix * model_matrix;

            // OO: Set bind group only once and update the buffer?
            context.queue_view_projection_matrix(&text_layer_matrix);

            let pass = &mut context.pass;
            pass.set_bind_group(0, context.view_projection_bind_group, &[]);

            pass.set_vertex_buffer(0, vertex_buffer.slice(..));

            pass.draw_indexed(
                0..(QuadIndexBuffer::QUAD_INDICES_COUNT * quad_count) as u32,
                0,
                0..1,
            )
        }
    }

    fn prepare_quads(
        &mut self,
        context: &mut PreparationContext,
        model_matrix: &Matrix4,
        // TODO: this double reference is quite unusual here
        // TODO: flatten!
        shapes: &[&QuadsShape],
    ) -> Result<Option<QuadsLayer>> {
        // Step 1: Get all instance data.
        // OO: Compute a conservative capacity?
        // OO: Use an iterator.
        // OO: We throw this away in this function further down below.
        let mut vertices = Vec::new();

        for QuadsShape { quads, .. } in shapes {
            for Quad {
                vertices: qv,
                color,
            } in quads
            {
                vertices.extend([
                    ColorVertex::new(qv[0], *color),
                    ColorVertex::new(qv[1], *color),
                    ColorVertex::new(qv[2], *color),
                    ColorVertex::new(qv[3], *color),
                ]);
            }
        }

        if vertices.is_empty() {
            return Ok(None);
        }

        let device = context.device;

        let vertex_buffer = device.create_buffer_init(&BufferInitDescriptor {
            label: Some("Quads Vertex Buffer"),
            contents: bytemuck::cast_slice(&vertices),
            usage: BufferUsages::VERTEX,
        });

        let quads_layer = QuadsLayer {
            model_matrix: *model_matrix,
            vertex_buffer,
            quad_count: vertices.len() >> 2,
        };

        Ok(Some(quads_layer))
    }
}