use std::sync::Arc;

use anyhow::Result;
use rayon::prelude::*;
use rust_wgpu_fiber::{
    ResourceName,
    eframe::{egui, egui_wgpu, wgpu},
    shader_space::ShaderSpace,
};

use crate::{
    app::frame::request_keys::{
        ClippingRequestKey, DiffRequestKey, DiffStatsRequestKey, QualifierRequestKey,
    },
    asset_store::AssetStore,
    dsl::SceneDSL,
    renderer,
    ui::{
        clipping_map::ClippingMapRenderer, diff_renderer::DiffRenderer,
        hdr_clamp::HdrClampRenderer, qualifier_map::QualifierMapRenderer,
    },
};

use super::types::{DiffStats, MatrixConfig, ResourcePoolInfo};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MatrixCellCoord {
    pub row: usize,
    pub col: usize,
}

pub struct MatrixCell {
    pub coord: MatrixCellCoord,
    pub display_coord: MatrixCellCoord,
    pub shader_space: ShaderSpace,
    pub output_texture_name: ResourceName,
    pub egui_texture_id: Option<egui::TextureId>,
    pub hdr_clamp_renderer: Option<HdrClampRenderer>,
    pub hdr_clamped_egui_id: Option<egui::TextureId>,
    pub pixel_cache: Option<super::canvas::pixel_overlay::PixelOverlayCache>,
    pub diff_renderer: Option<DiffRenderer>,
    pub diff_texture_id: Option<egui::TextureId>,
    pub last_diff_request_key: Option<DiffRequestKey>,
    pub last_diff_stats_request_key: Option<DiffStatsRequestKey>,
    pub diff_stats: Option<DiffStats>,
    pub clipping_renderer: Option<ClippingMapRenderer>,
    pub clipping_texture_id: Option<egui::TextureId>,
    pub last_clipping_request_key: Option<ClippingRequestKey>,
    pub qualifier_renderer: Option<QualifierMapRenderer>,
    pub qualifier_texture_id: Option<egui::TextureId>,
    pub last_qualifier_request_key: Option<QualifierRequestKey>,
}

pub struct MatrixRenderState {
    pub cells: Vec<MatrixCell>,
    pub logical_rows: usize,
    pub logical_cols: usize,
    pub grid_rows: usize,
    pub grid_cols: usize,
    pub row_chunks_per_logical_row: usize,
    pub show_labels: bool,
    pub cell_resolution: [u32; 2],
    pub row_pool_id: Option<String>,
    pub col_pool_id: Option<String>,
    pub base_pipeline_signature: Option<[u8; 32]>,
    pub hovered_coord: Option<MatrixCellCoord>,
    pub sticky_stats_coord: Option<MatrixCellCoord>,
}

impl Default for MatrixRenderState {
    fn default() -> Self {
        Self {
            cells: Vec::new(),
            logical_rows: 0,
            logical_cols: 0,
            grid_rows: 0,
            grid_cols: 0,
            row_chunks_per_logical_row: 0,
            show_labels: true,
            cell_resolution: [0, 0],
            row_pool_id: None,
            col_pool_id: None,
            base_pipeline_signature: None,
            hovered_coord: None,
            sticky_stats_coord: None,
        }
    }
}

impl MatrixRenderState {
    pub fn clear(&mut self, renderer: &mut egui_wgpu::Renderer) {
        for cell in self.cells.drain(..) {
            if let Some(id) = cell.egui_texture_id {
                renderer.free_texture(&id);
            }
            if let Some(id) = cell.hdr_clamped_egui_id {
                renderer.free_texture(&id);
            }
            if let Some(id) = cell.diff_texture_id {
                renderer.free_texture(&id);
            }
            if let Some(id) = cell.clipping_texture_id {
                renderer.free_texture(&id);
            }
            if let Some(id) = cell.qualifier_texture_id {
                renderer.free_texture(&id);
            }
        }
        self.logical_rows = 0;
        self.logical_cols = 0;
        self.grid_rows = 0;
        self.grid_cols = 0;
        self.row_chunks_per_logical_row = 0;
        self.show_labels = true;
        self.cell_resolution = [0, 0];
        self.row_pool_id = None;
        self.col_pool_id = None;
        self.base_pipeline_signature = None;
        self.hovered_coord = None;
        self.sticky_stats_coord = None;
    }

    pub fn stats_cell(&self) -> Option<&MatrixCell> {
        let coord = self
            .hovered_coord
            .or(self.sticky_stats_coord)
            .or_else(|| self.cells.first().map(|c| c.coord))?;
        self.cells.iter().find(|c| c.coord == coord)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MatrixDisplayLayout {
    display_rows: usize,
    display_cols: usize,
    row_chunks_per_logical_row: usize,
}

fn ceil_div(n: usize, d: usize) -> usize {
    if d == 0 { 0 } else { n.div_ceil(d) }
}

fn matrix_display_layout(
    logical_rows: usize,
    logical_cols: usize,
    max_row_cols: usize,
) -> MatrixDisplayLayout {
    if logical_rows == 0 || logical_cols == 0 {
        return MatrixDisplayLayout {
            display_rows: 0,
            display_cols: 0,
            row_chunks_per_logical_row: 0,
        };
    }

    let display_cols = if max_row_cols == 0 {
        logical_cols
    } else {
        max_row_cols.clamp(1, logical_cols)
    };
    let row_chunks_per_logical_row = ceil_div(logical_cols, display_cols).max(1);

    MatrixDisplayLayout {
        display_rows: logical_rows * row_chunks_per_logical_row,
        display_cols,
        row_chunks_per_logical_row,
    }
}

fn matrix_display_coord(coord: MatrixCellCoord, layout: MatrixDisplayLayout) -> MatrixCellCoord {
    if layout.display_cols == 0 {
        return coord;
    }

    MatrixCellCoord {
        row: coord.row * layout.row_chunks_per_logical_row + coord.col / layout.display_cols,
        col: coord.col % layout.display_cols,
    }
}

pub fn relayout_matrix(config: &MatrixConfig, state: &mut MatrixRenderState) {
    let layout = matrix_display_layout(state.logical_rows, state.logical_cols, config.max_row_cols);
    state.grid_rows = layout.display_rows;
    state.grid_cols = layout.display_cols;
    state.row_chunks_per_logical_row = layout.row_chunks_per_logical_row;
    state.show_labels = config.show_labels;

    for cell in &mut state.cells {
        cell.display_coord = matrix_display_coord(cell.coord, layout);
    }
}

pub struct MatrixBuildParams<'a> {
    pub scene: &'a SceneDSL,
    pub config: &'a MatrixConfig,
    pub resource_pools: &'a [ResourcePoolInfo],
    pub device: Arc<wgpu::Device>,
    pub queue: Arc<wgpu::Queue>,
    pub adapter: Option<&'a wgpu::Adapter>,
    pub asset_store: &'a AssetStore,
}

pub fn rebuild_matrix(
    params: MatrixBuildParams<'_>,
    render_state: &egui_wgpu::RenderState,
    renderer: &mut egui_wgpu::Renderer,
    state: &mut MatrixRenderState,
) -> Result<()> {
    state.clear(renderer);

    let selected_pools: Vec<&ResourcePoolInfo> = params
        .config
        .selected_pool_ids
        .iter()
        .filter_map(|id| params.resource_pools.iter().find(|p| p.node_id == *id))
        .collect();

    if selected_pools.is_empty() {
        return Ok(());
    }

    let (logical_rows, logical_cols, row_pool, col_pool) = match selected_pools.len() {
        1 => {
            let pool = selected_pools[0];
            (1usize, pool.item_count, None, Some(pool))
        }
        2 => {
            let row = selected_pools[0];
            let col = selected_pools[1];
            (row.item_count, col.item_count, Some(row), Some(col))
        }
        _ => return Ok(()),
    };

    state.logical_rows = logical_rows;
    state.logical_cols = logical_cols;
    state.row_pool_id = row_pool.map(|p| p.node_id.clone());
    state.col_pool_id = col_pool.map(|p| p.node_id.clone());

    let row_pool_id = row_pool.map(|p| &p.node_id);
    let col_pool_id = col_pool.map(|p| &p.node_id);

    let coords: Vec<(usize, usize)> = (0..logical_rows)
        .flat_map(|r| (0..logical_cols).map(move |c| (r, c)))
        .collect();

    struct BuiltCell {
        coord: MatrixCellCoord,
        shader_space: ShaderSpace,
        output_texture_name: ResourceName,
    }

    let built_cells: Vec<BuiltCell> = coords
        .par_iter()
        .filter_map(|&(row, col)| {
            let mut variant_scene = params.scene.clone();

            if let Some(rp_id) = row_pool_id {
                patch_pool_index(&mut variant_scene, rp_id, row as i64);
            }
            if let Some(cp_id) = col_pool_id {
                patch_pool_index(&mut variant_scene, cp_id, col as i64);
            }

            let mut builder =
                renderer::ShaderSpaceBuilder::new(params.device.clone(), params.queue.clone())
                    .with_options(renderer::ShaderSpaceBuildOptions {
                        presentation_mode: renderer::ShaderSpacePresentationMode::UiHdrNative,
                        ..Default::default()
                    })
                    .with_asset_store(params.asset_store.clone());
            if let Some(adapter) = params.adapter {
                builder = builder.with_adapter(adapter.clone());
            }

            match builder.build(&variant_scene) {
                Ok(result) => {
                    result.shader_space.render();
                    Some(BuiltCell {
                        coord: MatrixCellCoord { row, col },
                        shader_space: result.shader_space,
                        output_texture_name: result.present_output_texture,
                    })
                }
                Err(e) => {
                    eprintln!("[matrix] failed to build cell [{row}, {col}]: {e:#}");
                    None
                }
            }
        })
        .collect();

    for cell in built_cells {
        let egui_id = if let Some(tex) = cell
            .shader_space
            .textures
            .get(cell.output_texture_name.as_str())
        {
            if state.cell_resolution == [0, 0] {
                if let Some(info) = cell
                    .shader_space
                    .texture_info(cell.output_texture_name.as_str())
                {
                    state.cell_resolution = [info.size.width, info.size.height];
                }
            }
            tex.wgpu_texture_view.as_ref().map(|view| {
                renderer.register_native_texture_with_sampler_options(
                    &render_state.device,
                    view,
                    super::texture_bridge::canvas_sampler_descriptor(wgpu::FilterMode::Linear),
                )
            })
        } else {
            None
        };

        state.cells.push(MatrixCell {
            coord: cell.coord,
            display_coord: cell.coord,
            shader_space: cell.shader_space,
            output_texture_name: cell.output_texture_name,
            egui_texture_id: egui_id,
            hdr_clamp_renderer: None,
            hdr_clamped_egui_id: None,
            pixel_cache: None,
            diff_renderer: None,
            diff_texture_id: None,
            last_diff_request_key: None,
            last_diff_stats_request_key: None,
            diff_stats: None,
            clipping_renderer: None,
            clipping_texture_id: None,
            last_clipping_request_key: None,
            qualifier_renderer: None,
            qualifier_texture_id: None,
            last_qualifier_request_key: None,
        });
    }

    relayout_matrix(params.config, state);

    Ok(())
}

pub fn ensure_cell_pixel_cache(cell: &mut MatrixCell) {
    use super::canvas::pixel_overlay::{PixelOverlayCache, PixelOverlayReadback};

    if cell.pixel_cache.is_some() {
        return;
    }
    let Some(info) = cell
        .shader_space
        .texture_info(cell.output_texture_name.as_str())
    else {
        return;
    };
    let width = info.size.width;
    let height = info.size.height;
    let format = info.format;
    let readback = match format {
        wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb => cell
            .shader_space
            .read_texture_rgba8(cell.output_texture_name.as_str())
            .map(|image| PixelOverlayReadback::Rgba8(image.bytes))
            .unwrap_or(PixelOverlayReadback::Unavailable),
        wgpu::TextureFormat::Rgba16Float => cell
            .shader_space
            .read_texture_rgba16f(cell.output_texture_name.as_str())
            .map(|image| PixelOverlayReadback::Rgba16f(image.channels))
            .unwrap_or(PixelOverlayReadback::Unavailable),
        _ => PixelOverlayReadback::UnsupportedFormat,
    };
    cell.pixel_cache = Some(PixelOverlayCache {
        texture_name: cell.output_texture_name.as_str().to_string(),
        width,
        height,
        format,
        readback,
    });
}

#[cfg(test)]
mod tests {
    use super::{MatrixCellCoord, matrix_display_coord, matrix_display_layout};

    #[test]
    fn matrix_layout_is_unwrapped_when_max_cols_is_zero() {
        let layout = matrix_display_layout(2, 5, 0);

        assert_eq!(layout.display_rows, 2);
        assert_eq!(layout.display_cols, 5);
        assert_eq!(layout.row_chunks_per_logical_row, 1);
        assert_eq!(
            matrix_display_coord(MatrixCellCoord { row: 1, col: 4 }, layout),
            MatrixCellCoord { row: 1, col: 4 }
        );
    }

    #[test]
    fn matrix_layout_wraps_each_logical_row_at_max_cols() {
        let layout = matrix_display_layout(2, 5, 3);

        assert_eq!(layout.display_rows, 4);
        assert_eq!(layout.display_cols, 3);
        assert_eq!(layout.row_chunks_per_logical_row, 2);
        assert_eq!(
            matrix_display_coord(MatrixCellCoord { row: 0, col: 4 }, layout),
            MatrixCellCoord { row: 1, col: 1 }
        );
        assert_eq!(
            matrix_display_coord(MatrixCellCoord { row: 1, col: 4 }, layout),
            MatrixCellCoord { row: 3, col: 1 }
        );
    }
}

pub fn sync_matrix_filter(
    state: &mut MatrixRenderState,
    render_state: &egui_wgpu::RenderState,
    renderer: &mut egui_wgpu::Renderer,
    filter: wgpu::FilterMode,
) {
    for cell in &mut state.cells {
        let sampler = super::texture_bridge::canvas_sampler_descriptor(filter);
        if let Some(egui_id) = cell.egui_texture_id {
            if let Some(tex) = cell
                .shader_space
                .textures
                .get(cell.output_texture_name.as_str())
            {
                if let Some(view) = tex.wgpu_texture_view.as_ref() {
                    renderer.update_egui_texture_from_wgpu_texture_with_sampler_options(
                        &render_state.device,
                        view,
                        sampler.clone(),
                        egui_id,
                    );
                }
            }
        }
        if let Some(clamped_id) = cell.hdr_clamped_egui_id {
            if let Some(clamp_renderer) = cell.hdr_clamp_renderer.as_ref() {
                renderer.update_egui_texture_from_wgpu_texture_with_sampler_options(
                    &render_state.device,
                    clamp_renderer.output_view(),
                    sampler,
                    clamped_id,
                );
            }
        }
    }
}

pub fn sync_matrix_hdr_clamp(
    state: &mut MatrixRenderState,
    render_state: &egui_wgpu::RenderState,
    renderer: &mut egui_wgpu::Renderer,
    hdr_clamp_enabled: bool,
    filter: wgpu::FilterMode,
) {
    if !hdr_clamp_enabled {
        return;
    }
    for cell in &mut state.cells {
        let Some(tex) = cell
            .shader_space
            .textures
            .get(cell.output_texture_name.as_str())
        else {
            continue;
        };
        let Some(source_view) = tex.wgpu_texture_view.as_ref() else {
            continue;
        };
        let source_size = [
            tex.wgpu_texture_desc.size.width,
            tex.wgpu_texture_desc.size.height,
        ];

        let clamp_renderer = cell
            .hdr_clamp_renderer
            .get_or_insert_with(|| HdrClampRenderer::new(&render_state.device, source_size));
        clamp_renderer.update(
            &render_state.device,
            &render_state.queue,
            source_view,
            source_size,
        );

        let sampler = super::texture_bridge::canvas_sampler_descriptor(filter);
        if let Some(clamped_id) = cell.hdr_clamped_egui_id {
            renderer.update_egui_texture_from_wgpu_texture_with_sampler_options(
                &render_state.device,
                clamp_renderer.output_view(),
                sampler,
                clamped_id,
            );
        } else {
            cell.hdr_clamped_egui_id = Some(renderer.register_native_texture_with_sampler_options(
                &render_state.device,
                clamp_renderer.output_view(),
                sampler,
            ));
        }
    }
}

fn patch_pool_index(scene: &mut SceneDSL, pool_node_id: &str, index: i64) {
    let val = serde_json::json!(index);

    let origin_id = scene
        .nodes
        .iter()
        .find(|n| n.id == pool_node_id)
        .and_then(|n| n.params.get("__dedup_original_id"))
        .and_then(|v| v.as_str())
        .map(ToOwned::to_owned);

    for node in &mut scene.nodes {
        if node.node_type != "ResourcePool" {
            continue;
        }
        let matches = if let Some(ref oid) = origin_id {
            node.params
                .get("__dedup_original_id")
                .and_then(|v| v.as_str())
                == Some(oid.as_str())
        } else {
            node.id == pool_node_id
        };
        if matches {
            node.params.insert("selectedIndex".to_string(), val.clone());
        }
    }
}
