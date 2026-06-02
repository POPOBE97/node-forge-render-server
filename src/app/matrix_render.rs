use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use anyhow::Result;
use crossbeam_channel::{Receiver, TryRecvError};
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
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
    build_generation: u64,
    build_job: Option<MatrixBuildJob>,
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
            build_generation: 0,
            build_job: None,
        }
    }
}

impl MatrixRenderState {
    pub fn clear(&mut self, renderer: &mut egui_wgpu::Renderer) {
        self.cancel_build();
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

    fn cancel_build(&mut self) {
        if let Some(job) = self.build_job.take() {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }

    pub fn is_building(&self) -> bool {
        self.build_job.is_some()
    }

    pub fn stats_cell(&self) -> Option<&MatrixCell> {
        let coord = self
            .hovered_coord
            .or(self.sticky_stats_coord)
            .or_else(|| self.cells.first().map(|c| c.coord))?;
        self.cells.iter().find(|c| c.coord == coord)
    }
}

struct MatrixBuildJob {
    generation: u64,
    rx: Receiver<MatrixBuildMessage>,
    cancel: Arc<AtomicBool>,
    expected_cells: usize,
    completed_cells: usize,
    failed_cells: usize,
}

struct BuiltCell {
    coord: MatrixCellCoord,
    shader_space: ShaderSpace,
    output_texture_name: ResourceName,
}

enum MatrixBuildMessage {
    CellReady {
        generation: u64,
        cell: BuiltCell,
    },
    CellFailed {
        generation: u64,
        coord: MatrixCellCoord,
    },
    Finished {
        generation: u64,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MatrixPollResult {
    pub added_cells: usize,
    pub failed_cells: usize,
    pub finished: bool,
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

pub fn start_matrix_rebuild(
    params: MatrixBuildParams<'_>,
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

    relayout_matrix(params.config, state);

    if coords.is_empty() {
        return Ok(());
    }

    let generation = state.build_generation.wrapping_add(1);
    state.build_generation = generation;
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = cancel.clone();
    let (tx, rx) = crossbeam_channel::unbounded::<MatrixBuildMessage>();

    let scene = params.scene.clone();
    let device = params.device.clone();
    let queue = params.queue.clone();
    let adapter = params.adapter.cloned();
    let asset_store = params.asset_store.clone();
    let row_pool_id = row_pool_id.cloned();
    let col_pool_id = col_pool_id.cloned();
    let expected_cells = coords.len();

    thread::spawn(move || {
        coords
            .par_iter()
            .for_each_with(tx.clone(), |tx, &(row, col)| {
                if worker_cancel.load(Ordering::Relaxed) {
                    return;
                }

                let coord = MatrixCellCoord { row, col };
                match build_matrix_cell(
                    &scene,
                    row_pool_id.as_deref(),
                    col_pool_id.as_deref(),
                    coord,
                    device.clone(),
                    queue.clone(),
                    adapter.as_ref(),
                    asset_store.clone(),
                ) {
                    Ok(cell) if !worker_cancel.load(Ordering::Relaxed) => {
                        let _ = tx.send(MatrixBuildMessage::CellReady { generation, cell });
                    }
                    Ok(_) => {}
                    Err(e) => {
                        eprintln!("[matrix] failed to build cell [{row}, {col}]: {e:#}");
                        if !worker_cancel.load(Ordering::Relaxed) {
                            let _ = tx.send(MatrixBuildMessage::CellFailed { generation, coord });
                        }
                    }
                }
            });
        let _ = tx.send(MatrixBuildMessage::Finished { generation });
    });

    state.build_job = Some(MatrixBuildJob {
        generation,
        rx,
        cancel,
        expected_cells,
        completed_cells: 0,
        failed_cells: 0,
    });

    Ok(())
}

fn build_matrix_cell(
    scene: &SceneDSL,
    row_pool_id: Option<&str>,
    col_pool_id: Option<&str>,
    coord: MatrixCellCoord,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    adapter: Option<&wgpu::Adapter>,
    asset_store: AssetStore,
) -> Result<BuiltCell> {
    let mut variant_scene = scene.clone();

    if let Some(rp_id) = row_pool_id {
        patch_pool_index(&mut variant_scene, rp_id, coord.row as i64);
    }
    if let Some(cp_id) = col_pool_id {
        patch_pool_index(&mut variant_scene, cp_id, coord.col as i64);
    }

    let mut builder = renderer::ShaderSpaceBuilder::new(device, queue)
        .with_options(renderer::ShaderSpaceBuildOptions {
            presentation_mode: renderer::ShaderSpacePresentationMode::UiHdrNative,
            ..Default::default()
        })
        .with_asset_store(asset_store);
    if let Some(adapter) = adapter {
        builder = builder.with_adapter(adapter.clone());
    }

    let result = builder.build(&variant_scene)?;
    result.shader_space.render();
    Ok(BuiltCell {
        coord,
        shader_space: result.shader_space,
        output_texture_name: result.present_output_texture,
    })
}

const MAX_READY_CELLS_PER_FRAME: usize = 16;

pub fn poll_matrix_rebuild(
    state: &mut MatrixRenderState,
    render_state: &egui_wgpu::RenderState,
    renderer: &mut egui_wgpu::Renderer,
    filter: wgpu::FilterMode,
    hdr_clamp_enabled: bool,
) -> MatrixPollResult {
    let Some(active_generation) = state.build_job.as_ref().map(|job| job.generation) else {
        return MatrixPollResult::default();
    };

    let mut messages = Vec::new();
    let mut ready_messages = 0usize;
    let mut disconnected = false;

    if let Some(job) = state.build_job.as_ref() {
        loop {
            match job.rx.try_recv() {
                Ok(message) => {
                    if matches!(message, MatrixBuildMessage::CellReady { .. }) {
                        ready_messages += 1;
                    }
                    messages.push(message);
                    if ready_messages >= MAX_READY_CELLS_PER_FRAME {
                        break;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            }
        }
    }

    let mut result = MatrixPollResult::default();
    let mut saw_finished = disconnected;

    for message in messages {
        match message {
            MatrixBuildMessage::CellReady { generation, cell }
                if generation == active_generation =>
            {
                if let Some(matrix_cell) =
                    register_built_cell(state, render_state, renderer, cell, filter)
                {
                    insert_cell_sorted(&mut state.cells, matrix_cell);
                    result.added_cells += 1;
                } else {
                    result.failed_cells += 1;
                }
            }
            MatrixBuildMessage::CellFailed { generation, coord }
                if generation == active_generation =>
            {
                let _ = coord;
                result.failed_cells += 1;
            }
            MatrixBuildMessage::Finished { generation } if generation == active_generation => {
                saw_finished = true;
            }
            MatrixBuildMessage::CellReady { .. }
            | MatrixBuildMessage::CellFailed { .. }
            | MatrixBuildMessage::Finished { .. } => {}
        }
    }

    let mut clear_job = false;
    if let Some(job) = state.build_job.as_mut()
        && job.generation == active_generation
    {
        job.completed_cells += result.added_cells;
        job.failed_cells += result.failed_cells;
        clear_job = saw_finished || job.completed_cells + job.failed_cells >= job.expected_cells;
    }
    if clear_job {
        state.build_job = None;
        result.finished = true;
    }

    if result.added_cells > 0 && hdr_clamp_enabled {
        sync_matrix_hdr_clamp(state, render_state, renderer, true, filter);
    }

    result
}

fn register_built_cell(
    state: &mut MatrixRenderState,
    render_state: &egui_wgpu::RenderState,
    renderer: &mut egui_wgpu::Renderer,
    cell: BuiltCell,
    filter: wgpu::FilterMode,
) -> Option<MatrixCell> {
    let tex = cell
        .shader_space
        .textures
        .get(cell.output_texture_name.as_str())?;
    if state.cell_resolution == [0, 0] {
        if let Some(info) = cell
            .shader_space
            .texture_info(cell.output_texture_name.as_str())
        {
            state.cell_resolution = [info.size.width, info.size.height];
        }
    }
    let view = tex.wgpu_texture_view.as_ref()?;
    let egui_id = renderer.register_native_texture_with_sampler_options(
        &render_state.device,
        view,
        super::texture_bridge::canvas_sampler_descriptor(filter),
    );

    let layout = MatrixDisplayLayout {
        display_rows: state.grid_rows,
        display_cols: state.grid_cols,
        row_chunks_per_logical_row: state.row_chunks_per_logical_row,
    };

    Some(MatrixCell {
        coord: cell.coord,
        display_coord: matrix_display_coord(cell.coord, layout),
        shader_space: cell.shader_space,
        output_texture_name: cell.output_texture_name,
        egui_texture_id: Some(egui_id),
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
    })
}

fn insert_cell_sorted(cells: &mut Vec<MatrixCell>, cell: MatrixCell) {
    match cells.binary_search_by_key(&cell.coord, |existing| existing.coord) {
        Ok(pos) => {
            cells[pos] = cell;
        }
        Err(pos) => cells.insert(pos, cell),
    }
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

    #[test]
    fn matrix_cell_coord_sort_is_row_major() {
        let mut coords = vec![
            MatrixCellCoord { row: 1, col: 0 },
            MatrixCellCoord { row: 0, col: 2 },
            MatrixCellCoord { row: 0, col: 0 },
            MatrixCellCoord { row: 1, col: 1 },
        ];

        coords.sort();

        assert_eq!(
            coords,
            vec![
                MatrixCellCoord { row: 0, col: 0 },
                MatrixCellCoord { row: 0, col: 2 },
                MatrixCellCoord { row: 1, col: 0 },
                MatrixCellCoord { row: 1, col: 1 },
            ]
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
