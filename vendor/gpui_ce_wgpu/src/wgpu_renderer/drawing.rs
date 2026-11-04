use gpui::{
    AtlasTextureId, MonochromeSprite, Path, PolychromeSprite, Quad, ScaledPixels, Shadow,
    SubpixelSprite, Underline,
};

use crate::WgpuTextureInfo;
use gpui_render::shaders::interface::{self as shader_interface, BufferData};

use super::{
    WgpuRenderer,
    buffers::{InstanceSlice, InstanceUpload},
    frame, path_types, pipelines,
};

impl WgpuRenderer {
    pub(super) fn draw_quads(
        &self,
        quads: &[Quad],
        smoothed: bool,
        instances: &mut InstanceUpload,
        pass: &mut wgpu::RenderPass<'_>,
    ) -> frame::DrawResult {
        let pipelines = &self.resources().pipelines;
        let pipeline = if smoothed {
            &pipelines.smoothed_quads
        } else {
            &pipelines.quads
        };
        // Shed patch: a large quad that is only a border (a card's outline)
        // ran the quad shader over its whole area. Drawn as four edge strips
        // of the same quad, each clipped to its strip (gpui_ce_render draws a
        // quad only inside its clip), it costs its border.
        if quads.iter().any(|q| strips(q).is_some()) {
            let split: Vec<Quad> = quads
                .iter()
                .flat_map(|q| match strips(q) {
                    Some(strips) => strips.into_iter().flatten().collect::<Vec<_>>(),
                    None => vec![q.clone()],
                })
                .collect();
            return self.draw_instances(&split, pipeline, instances, pass);
        }
        self.draw_instances(quads, pipeline, instances, pass)
    }

    pub(super) fn draw_shadows(
        &self,
        shadows: &[Shadow],
        smoothed: bool,
        instances: &mut InstanceUpload,
        pass: &mut wgpu::RenderPass<'_>,
    ) -> frame::DrawResult {
        let pipelines = &self.resources().pipelines;
        let pipeline = if smoothed {
            &pipelines.smoothed_shadows
        } else {
            &pipelines.shadows
        };
        self.draw_instances(shadows, pipeline, instances, pass)
    }

    pub(super) fn draw_underlines(
        &self,
        underlines: &[Underline],
        instances: &mut InstanceUpload,
        pass: &mut wgpu::RenderPass<'_>,
    ) -> frame::DrawResult {
        self.draw_instances(
            underlines,
            &self.resources().pipelines.underlines,
            instances,
            pass,
        )
    }

    pub(super) fn draw_monochrome_sprites(
        &self,
        sprites: &[MonochromeSprite],
        texture_id: AtlasTextureId,
        instances: &mut InstanceUpload,
        pass: &mut wgpu::RenderPass<'_>,
    ) -> frame::DrawResult {
        if sprites.is_empty() {
            return Ok(());
        }
        let Some(texture) = self.atlas.get_texture_info(texture_id) else {
            return Ok(());
        };
        self.draw_instances_with_texture(
            sprites,
            texture_id,
            &texture,
            &self.resources().pipelines.monochrome_sprites,
            instances,
            pass,
        )
    }

    pub(super) fn draw_subpixel_sprites(
        &self,
        sprites: &[SubpixelSprite],
        texture_id: AtlasTextureId,
        instances: &mut InstanceUpload,
        pass: &mut wgpu::RenderPass<'_>,
    ) -> frame::DrawResult {
        if sprites.is_empty() {
            return Ok(());
        }
        let Some(texture) = self.atlas.get_texture_info(texture_id) else {
            return Ok(());
        };
        let resources = self.resources();
        let pipeline = resources
            .pipelines
            .subpixel_sprites
            .as_ref()
            .unwrap_or(&resources.pipelines.monochrome_sprites);
        self.draw_instances_with_texture(sprites, texture_id, &texture, pipeline, instances, pass)
    }

    pub(super) fn draw_polychrome_sprites(
        &self,
        sprites: &[PolychromeSprite],
        texture_id: AtlasTextureId,
        smoothed: bool,
        instances: &mut InstanceUpload,
        pass: &mut wgpu::RenderPass<'_>,
    ) -> frame::DrawResult {
        if sprites.is_empty() {
            return Ok(());
        }
        let Some(texture) = self.atlas.get_texture_info(texture_id) else {
            return Ok(());
        };
        let pipelines = &self.resources().pipelines;
        let pipeline = if smoothed {
            &pipelines.smoothed_polychrome_sprites
        } else {
            &pipelines.polychrome_sprites
        };
        self.draw_instances_with_texture(sprites, texture_id, &texture, pipeline, instances, pass)
    }

    fn draw_instances<T: BufferData>(
        &self,
        values: &[T],
        pipeline: &pipelines::WgpuRenderPipeline,
        instances: &mut InstanceUpload,
        pass: &mut wgpu::RenderPass<'_>,
    ) -> frame::DrawResult {
        if values.is_empty() {
            return Ok(());
        }
        let resources = self.resources();
        self.draw_bound_instances(
            values,
            pipeline,
            resources.instances.bind_group(),
            instances,
            pass,
        )
    }

    fn draw_bound_instances<T: BufferData>(
        &self,
        values: &[T],
        pipeline: &pipelines::WgpuRenderPipeline,
        bind_group: &wgpu::BindGroup,
        instances: &mut InstanceUpload,
        pass: &mut wgpu::RenderPass<'_>,
    ) -> frame::DrawResult {
        let Some(slice) = instances.write(values) else {
            return Err(frame::DrawError::CapacityPlanningInvariant);
        };
        self.draw_bound_slice(pipeline, bind_group, &slice, pass);
        Ok(())
    }

    fn draw_bound_slice<T>(
        &self,
        pipeline: &pipelines::WgpuRenderPipeline,
        bind_group: &wgpu::BindGroup,
        slice: &InstanceSlice<T>,
        pass: &mut wgpu::RenderPass<'_>,
    ) {
        pass.set_pipeline(pipeline);
        slice.set_data_bind_group(pass, bind_group);
        pass.draw(0..pipeline.fixed_vertex_count(), slice.range());
    }

    fn draw_instances_with_texture<T: BufferData>(
        &self,
        values: &[T],
        texture_id: AtlasTextureId,
        texture: &WgpuTextureInfo,
        pipeline: &pipelines::WgpuRenderPipeline,
        instances: &mut InstanceUpload,
        pass: &mut wgpu::RenderPass<'_>,
    ) -> frame::DrawResult {
        if values.is_empty() {
            return Ok(());
        }
        let resources = self.resources();
        let bind_group = resources.instances.textured_bind_group(
            &resources.device,
            &resources.bind_group_layouts,
            pipeline.data_layout(),
            texture_id,
            texture.identity,
            &texture.view,
            &resources.atlas_sampler,
        );
        self.draw_bound_instances(values, pipeline, &bind_group, instances, pass)
    }

    pub(super) fn draw_paths_from_intermediate(
        &self,
        paths: &[Path<ScaledPixels>],
        instances: &mut InstanceUpload,
        pass: &mut wgpu::RenderPass<'_>,
    ) -> frame::DrawResult {
        let sprite_count = path_types::sprite_count(paths);
        let Some(sprite_slice) = instances.write_iter(sprite_count, path_types::sprites(paths))
        else {
            return Err(frame::DrawError::CapacityPlanningInvariant);
        };
        let resources = self.resources();
        let Some(path_intermediate_view) = resources.path_intermediate_view.as_ref() else {
            return Err(frame::DrawError::MissingIntermediateTarget);
        };
        let bind_group = resources.instances.path_bind_group(
            &resources.device,
            &resources.bind_group_layouts,
            resources.pipelines.paths.data_layout(),
            path_intermediate_view,
            &resources.atlas_sampler,
        );
        self.draw_bound_slice(&resources.pipelines.paths, &bind_group, &sprite_slice, pass);
        Ok(())
    }

    pub(super) fn draw_paths_to_intermediate(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        paths: &[Path<ScaledPixels>],
        instances: &mut InstanceUpload,
    ) -> frame::DrawResult {
        let vertex_count = path_types::rasterization_vertex_count(paths);
        if vertex_count == 0 {
            return Ok(());
        }

        let Some(vertex_slice) =
            instances.write_iter(vertex_count, path_types::rasterization_vertices(paths))
        else {
            return Err(frame::DrawError::CapacityPlanningInvariant);
        };
        let resources = self.resources();
        let Some(path_intermediate_view) = resources.path_intermediate_view.as_ref() else {
            return Err(frame::DrawError::MissingIntermediateTarget);
        };
        let (target_view, resolve_target) = if let Some(msaa_view) = &resources.path_msaa_view {
            (msaa_view, Some(path_intermediate_view))
        } else {
            (path_intermediate_view, None)
        };

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("path_rasterization_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target_view,
                resolve_target,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    // Only the resolved texture is sampled later. The MSAA
                    // attachment is transient and cannot use StoreOp::Store.
                    store: if resolve_target.is_some() {
                        wgpu::StoreOp::Discard
                    } else {
                        wgpu::StoreOp::Store
                    },
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            ..Default::default()
        });
        pass.set_pipeline(&resources.pipelines.path_rasterization);
        pass.set_bind_group(
            shader_interface::GLOBAL_BIND_GROUP,
            &resources.path_globals_bind_group,
            &[],
        );
        vertex_slice.set_data_bind_group(&mut pass, resources.instances.bind_group());
        pass.draw(vertex_slice.range(), 0..1);
        Ok(())
    }
}

/// Shed patch: a border-only quad as four copies, each clipped to one edge
/// strip (wide enough for the border, its corner radii and antialiasing).
/// `None` for quads with a fill, a faded clip (narrowing it would move the
/// fade), or too small for the split to save anything.
pub(super) fn strips(quad: &Quad) -> Option<[Option<Quad>; 4]> {
    const MIN_AREA: f32 = 64.0 * 64.0;
    let b = quad.bounds;
    let mask = quad.content_mask.bounds;
    let fade = &quad.content_mask.fade_out;
    if !quad.background.is_transparent()
        || b.size.width.0 * b.size.height.0 < MIN_AREA
        || [fade.top, fade.right, fade.bottom, fade.left].iter().any(|f| f.0 != 0.0)
    {
        return None;
    }
    let (w, r) = (&quad.border_widths, &quad.corner_radii);
    if [w.top, w.right, w.bottom, w.left].iter().all(|e| e.0 == 0.0) {
        return None;
    }
    let pad = 2.0;
    let top = w.top.0.max(r.top_left.0).max(r.top_right.0) + pad;
    let bottom = w.bottom.0.max(r.bottom_left.0).max(r.bottom_right.0) + pad;
    let left = w.left.0.max(r.top_left.0).max(r.bottom_left.0) + pad;
    let right = w.right.0.max(r.top_right.0).max(r.bottom_right.0) + pad;
    let (x, y, width, height) = (b.origin.x.0, b.origin.y.0, b.size.width.0, b.size.height.0);
    if top + bottom >= height || left + right >= width {
        return None;
    }
    let strip = |sx: f32, sy: f32, sw: f32, sh: f32| -> Option<Quad> {
        let x0 = sx.max(mask.origin.x.0);
        let y0 = sy.max(mask.origin.y.0);
        let x1 = (sx + sw).min(mask.origin.x.0 + mask.size.width.0);
        let y1 = (sy + sh).min(mask.origin.y.0 + mask.size.height.0);
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        let mut q = quad.clone();
        q.content_mask.bounds = gpui::Bounds {
            origin: gpui::point(ScaledPixels(x0), ScaledPixels(y0)),
            size: gpui::size(ScaledPixels(x1 - x0), ScaledPixels(y1 - y0)),
        };
        Some(q)
    };
    let middle = height - top - bottom;
    Some([
        strip(x, y, width, top),
        strip(x, y + height - bottom, width, bottom),
        strip(x, y + top, left, middle),
        strip(x + width - right, y + top, right, middle),
    ])
}

/// Shed patch: how many instances a quad takes at most, split or not.
pub(super) fn quad_instances(quad: &Quad) -> usize {
    if strips(quad).is_some() { 4 } else { 1 }
}
