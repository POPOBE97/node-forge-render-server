use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Instant,
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
    state_machine::OverrideKey,
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
    pub variant_scene: SceneDSL,
    pub shader_space: ShaderSpace,
    pub pass_bindings: Vec<renderer::PassBindings>,
    pub pipeline_signature: [u8; 32],
    pub output_texture_name: ResourceName,
    pub dynamic_sync_pending: bool,
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
    pub matrix_uniform_refresh_count: u64,
    pub matrix_full_rebuild_count: u64,
    pub matrix_uniform_refresh_ms: f64,
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
            matrix_uniform_refresh_count: 0,
            matrix_full_rebuild_count: 0,
            matrix_uniform_refresh_ms: 0.0,
            build_generation: 0,
            build_job: None,
        }
    }
}

impl MatrixRenderState {
    pub fn clear(&mut self, renderer: &mut egui_wgpu::Renderer) {
        self.cancel_build(renderer);
        free_matrix_cells(renderer, &mut self.cells);
        self.reset_visible_state();
    }

    fn reset_visible_state(&mut self) {
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

    fn cancel_build(&mut self, _renderer: &mut egui_wgpu::Renderer) {
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
    pending: PendingMatrixState,
}

struct BuiltCell {
    coord: MatrixCellCoord,
    variant_scene: SceneDSL,
    shader_space: ShaderSpace,
    pass_bindings: Vec<renderer::PassBindings>,
    pipeline_signature: [u8; 32],
    output_texture_name: ResourceName,
}

struct PendingMatrixState {
    logical_rows: usize,
    logical_cols: usize,
    grid_rows: usize,
    grid_cols: usize,
    row_chunks_per_logical_row: usize,
    show_labels: bool,
    cell_resolution: [u32; 2],
    row_pool_id: Option<String>,
    col_pool_id: Option<String>,
    base_pipeline_signature: Option<[u8; 32]>,
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

pub enum MatrixUniformRefreshResult {
    Refreshed,
    NeedsFullRebuild,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MatrixDynamicRenderResult {
    pub rendered_cells: usize,
    pub failed_cells: usize,
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

fn apply_layout_to_matrix_cells(
    logical_rows: usize,
    logical_cols: usize,
    config: &MatrixConfig,
    grid_rows: &mut usize,
    grid_cols: &mut usize,
    row_chunks_per_logical_row: &mut usize,
    show_labels: &mut bool,
    cells: &mut [MatrixCell],
) {
    let layout = matrix_display_layout(logical_rows, logical_cols, config.max_row_cols);
    *grid_rows = layout.display_rows;
    *grid_cols = layout.display_cols;
    *row_chunks_per_logical_row = layout.row_chunks_per_logical_row;
    *show_labels = config.show_labels;

    for cell in cells {
        cell.display_coord = matrix_display_coord(cell.coord, layout);
    }
}

pub fn relayout_matrix(config: &MatrixConfig, state: &mut MatrixRenderState) {
    apply_layout_to_matrix_cells(
        state.logical_rows,
        state.logical_cols,
        config,
        &mut state.grid_rows,
        &mut state.grid_cols,
        &mut state.row_chunks_per_logical_row,
        &mut state.show_labels,
        &mut state.cells,
    );

    if let Some(job) = state.build_job.as_mut() {
        let layout = matrix_display_layout(
            job.pending.logical_rows,
            job.pending.logical_cols,
            config.max_row_cols,
        );
        job.pending.grid_rows = layout.display_rows;
        job.pending.grid_cols = layout.display_cols;
        job.pending.row_chunks_per_logical_row = layout.row_chunks_per_logical_row;
        job.pending.show_labels = config.show_labels;
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

fn free_matrix_cell(renderer: &mut egui_wgpu::Renderer, cell: MatrixCell) {
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

fn free_matrix_cells(renderer: &mut egui_wgpu::Renderer, cells: &mut Vec<MatrixCell>) {
    for cell in cells.drain(..) {
        free_matrix_cell(renderer, cell);
    }
}

fn visible_coord_within_bounds(
    coord: MatrixCellCoord,
    logical_rows: usize,
    logical_cols: usize,
) -> bool {
    coord.row < logical_rows && coord.col < logical_cols
}

fn prune_visible_cells_outside_bounds(
    renderer: &mut egui_wgpu::Renderer,
    cells: &mut Vec<MatrixCell>,
    logical_rows: usize,
    logical_cols: usize,
) {
    let mut retained = Vec::with_capacity(cells.len());
    for cell in cells.drain(..) {
        if visible_coord_within_bounds(cell.coord, logical_rows, logical_cols) {
            retained.push(cell);
        } else {
            free_matrix_cell(renderer, cell);
        }
    }
    *cells = retained;
}

fn apply_pending_visible_state(
    state: &mut MatrixRenderState,
    renderer: &mut egui_wgpu::Renderer,
    pending: &PendingMatrixState,
    config: &MatrixConfig,
) {
    prune_visible_cells_outside_bounds(
        renderer,
        &mut state.cells,
        pending.logical_rows,
        pending.logical_cols,
    );
    state.logical_rows = pending.logical_rows;
    state.logical_cols = pending.logical_cols;
    state.grid_rows = pending.grid_rows;
    state.grid_cols = pending.grid_cols;
    state.row_chunks_per_logical_row = pending.row_chunks_per_logical_row;
    state.show_labels = pending.show_labels;
    state.row_pool_id = pending.row_pool_id.clone();
    state.col_pool_id = pending.col_pool_id.clone();
    state.base_pipeline_signature = pending.base_pipeline_signature;
    if state.cells.is_empty() {
        state.cell_resolution = pending.cell_resolution;
    }
    apply_layout_to_matrix_cells(
        state.logical_rows,
        state.logical_cols,
        config,
        &mut state.grid_rows,
        &mut state.grid_cols,
        &mut state.row_chunks_per_logical_row,
        &mut state.show_labels,
        &mut state.cells,
    );

    if let Some(coord) = state.hovered_coord
        && !state.cells.iter().any(|cell| cell.coord == coord)
    {
        state.hovered_coord = None;
    }
    if let Some(coord) = state.sticky_stats_coord
        && !state.cells.iter().any(|cell| cell.coord == coord)
    {
        state.sticky_stats_coord = None;
    }
}

pub fn start_matrix_rebuild(
    params: MatrixBuildParams<'_>,
    renderer: &mut egui_wgpu::Renderer,
    state: &mut MatrixRenderState,
) -> Result<()> {
    state.cancel_build(renderer);

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

    let pending_row_pool_id = row_pool.map(|p| p.node_id.clone());
    let pending_col_pool_id = col_pool.map(|p| p.node_id.clone());

    let row_pool_id = row_pool.map(|p| &p.node_id);
    let col_pool_id = col_pool.map(|p| &p.node_id);

    let coords: Vec<(usize, usize)> = (0..logical_rows)
        .flat_map(|r| (0..logical_cols).map(move |c| (r, c)))
        .collect();

    relayout_matrix(params.config, state);

    if coords.is_empty() {
        state.clear(renderer);
        return Ok(());
    }

    let layout = matrix_display_layout(logical_rows, logical_cols, params.config.max_row_cols);
    let pending = PendingMatrixState {
        logical_rows,
        logical_cols,
        grid_rows: layout.display_rows,
        grid_cols: layout.display_cols,
        row_chunks_per_logical_row: layout.row_chunks_per_logical_row,
        show_labels: params.config.show_labels,
        cell_resolution: [0, 0],
        row_pool_id: pending_row_pool_id,
        col_pool_id: pending_col_pool_id,
        base_pipeline_signature: None,
    };
    apply_pending_visible_state(state, renderer, &pending, params.config);

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
        pending,
    });
    state.matrix_full_rebuild_count = state.matrix_full_rebuild_count.saturating_add(1);

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
        variant_scene,
        shader_space: result.shader_space,
        pass_bindings: result.pass_bindings,
        pipeline_signature: result.pipeline_signature,
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
                let mut ready_cell = None;
                let mut ready_resolution = None;
                if let Some(job) = state.build_job.as_mut()
                    && job.generation == active_generation
                {
                    if let Some(matrix_cell) = register_built_cell(
                        &mut job.pending.cell_resolution,
                        job.pending.grid_rows,
                        job.pending.grid_cols,
                        job.pending.row_chunks_per_logical_row,
                        render_state,
                        renderer,
                        cell,
                        filter,
                    ) {
                        ready_resolution = Some(job.pending.cell_resolution);
                        ready_cell = Some(matrix_cell);
                    } else {
                        result.failed_cells += 1;
                    }
                }
                if let Some(matrix_cell) = ready_cell {
                    if let Some(resolution) = ready_resolution
                        && resolution != [0, 0]
                    {
                        state.cell_resolution = resolution;
                    }
                    if state.base_pipeline_signature.is_none() {
                        state.base_pipeline_signature = Some(matrix_cell.pipeline_signature);
                    }
                    upsert_visible_cell(renderer, &mut state.cells, matrix_cell);
                    result.added_cells += 1;
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
        let _ = state.build_job.take();
        result.finished = true;
    }

    if (result.added_cells > 0 || result.finished) && hdr_clamp_enabled {
        sync_matrix_hdr_clamp(state, render_state, renderer, true, filter);
    }

    result
}

fn register_built_cell(
    cell_resolution: &mut [u32; 2],
    grid_rows: usize,
    grid_cols: usize,
    row_chunks_per_logical_row: usize,
    render_state: &egui_wgpu::RenderState,
    renderer: &mut egui_wgpu::Renderer,
    cell: BuiltCell,
    filter: wgpu::FilterMode,
) -> Option<MatrixCell> {
    let tex = cell
        .shader_space
        .textures
        .get(cell.output_texture_name.as_str())?;
    if *cell_resolution == [0, 0] {
        if let Some(info) = cell
            .shader_space
            .texture_info(cell.output_texture_name.as_str())
        {
            *cell_resolution = [info.size.width, info.size.height];
        }
    }
    let view = tex.wgpu_texture_view.as_ref()?;
    let egui_id = renderer.register_native_texture_with_sampler_options(
        &render_state.device,
        view,
        super::texture_bridge::canvas_sampler_descriptor(filter),
    );

    let layout = MatrixDisplayLayout {
        display_rows: grid_rows,
        display_cols: grid_cols,
        row_chunks_per_logical_row,
    };

    Some(MatrixCell {
        coord: cell.coord,
        display_coord: matrix_display_coord(cell.coord, layout),
        variant_scene: cell.variant_scene,
        shader_space: cell.shader_space,
        pass_bindings: cell.pass_bindings,
        pipeline_signature: cell.pipeline_signature,
        output_texture_name: cell.output_texture_name,
        dynamic_sync_pending: true,
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

fn upsert_visible_cell(
    renderer: &mut egui_wgpu::Renderer,
    cells: &mut Vec<MatrixCell>,
    cell: MatrixCell,
) {
    match cells.binary_search_by_key(&cell.coord, |existing| existing.coord) {
        Ok(pos) => {
            let previous = std::mem::replace(&mut cells[pos], cell);
            free_matrix_cell(renderer, previous);
        }
        Err(pos) => cells.insert(pos, cell),
    }
}

fn invalidate_matrix_cell_outputs(cell: &mut MatrixCell) {
    cell.pixel_cache = None;
    cell.last_diff_request_key = None;
    cell.last_diff_stats_request_key = None;
    cell.diff_stats = None;
    cell.last_clipping_request_key = None;
    cell.last_qualifier_request_key = None;
}

fn apply_frame_uniform_values(
    scene: &mut SceneDSL,
    values: &HashMap<OverrideKey, serde_json::Value>,
) -> Result<usize> {
    let mut changed = 0usize;
    for (key, value) in values {
        let Some(node) = scene.nodes.iter_mut().find(|node| node.id == key.node_id) else {
            // Each Matrix variation contains only the declarations materialized by
            // that Render Graph branch. A globally valid presentation key can
            // therefore be absent from a particular cell. Keep projection exact:
            // never suffix-match or reverse-resolve a consumer/group-internal ID.
            continue;
        };
        if node.params.get(&key.param_name) != Some(value) {
            node.params.insert(key.param_name.clone(), value.clone());
            changed += 1;
        }
    }
    Ok(changed)
}

pub fn render_matrix_dynamic_frame(
    state: &mut MatrixRenderState,
    frame_uniform_values: &HashMap<OverrideKey, serde_json::Value>,
    time_secs: f32,
    render_all_cells: bool,
    render_state: &egui_wgpu::RenderState,
    renderer: &mut egui_wgpu::Renderer,
    filter: wgpu::FilterMode,
    hdr_clamp_enabled: bool,
) -> MatrixDynamicRenderResult {
    let mut result = MatrixDynamicRenderResult::default();

    for cell in &mut state.cells {
        if !render_all_cells && !cell.dynamic_sync_pending {
            continue;
        }
        let render_result = (|| -> Result<()> {
            let mut frame_scene = cell.variant_scene.clone();
            apply_frame_uniform_values(&mut frame_scene, frame_uniform_values)?;
            super::scene_runtime::apply_graph_uniform_updates_parts(
                &mut cell.pass_bindings,
                &mut cell.shader_space,
                &frame_scene,
            )?;
            for pass in &mut cell.pass_bindings {
                let mut params = pass.base_params;
                params.time = time_secs;
                renderer::update_pass_params(&cell.shader_space, pass, &params)?;
            }
            cell.shader_space.render();
            Ok(())
        })();

        match render_result {
            Ok(()) => {
                invalidate_matrix_cell_outputs(cell);
                cell.dynamic_sync_pending = false;
                result.rendered_cells += 1;
            }
            Err(error) => {
                result.failed_cells += 1;
                eprintln!(
                    "[matrix] failed to render dynamic cell [{}, {}]: {error:#}",
                    cell.coord.row, cell.coord.col
                );
            }
        }
    }

    if result.rendered_cells > 0 && hdr_clamp_enabled {
        sync_matrix_hdr_clamp(state, render_state, renderer, true, filter);
    }

    result
}

pub fn refresh_matrix_cells_uniform_only(
    state: &mut MatrixRenderState,
    updated_nodes: &[crate::dsl::Node],
    render_state: &egui_wgpu::RenderState,
    renderer: &mut egui_wgpu::Renderer,
    filter: wgpu::FilterMode,
    hdr_clamp_enabled: bool,
) -> Result<MatrixUniformRefreshResult> {
    if state.cells.is_empty() || state.is_building() {
        return Ok(MatrixUniformRefreshResult::NeedsFullRebuild);
    }

    let started = Instant::now();
    for cell in &mut state.cells {
        super::scene_runtime::apply_uniform_node_param_updates(
            &mut cell.variant_scene,
            updated_nodes,
            true,
        )?;

        let next_signature = renderer::graph_uniforms::compute_pipeline_signature_for_pass_bindings(
            &cell.variant_scene,
            &cell.pass_bindings,
        );
        if next_signature != cell.pipeline_signature {
            anyhow::bail!(
                "matrix cell [{}, {}] pipeline signature changed during uniform refresh",
                cell.coord.row,
                cell.coord.col
            );
        }

        let _ = super::scene_runtime::apply_graph_uniform_updates_parts(
            &mut cell.pass_bindings,
            &mut cell.shader_space,
            &cell.variant_scene,
        )?;
        cell.shader_space.render();
        invalidate_matrix_cell_outputs(cell);
    }

    if hdr_clamp_enabled {
        sync_matrix_hdr_clamp(state, render_state, renderer, true, filter);
    }

    state.matrix_uniform_refresh_count = state.matrix_uniform_refresh_count.saturating_add(1);
    state.matrix_uniform_refresh_ms += started.elapsed().as_secs_f64() * 1000.0;

    Ok(MatrixUniformRefreshResult::Refreshed)
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
    use std::collections::HashMap;

    use super::{
        MatrixCellCoord, apply_frame_uniform_values, matrix_display_coord, matrix_display_layout,
    };
    use crate::{
        dsl::{Metadata, Node, SceneDSL},
        state_machine::OverrideKey,
    };

    fn frame_value_scene() -> SceneDSL {
        SceneDSL {
            version: "1.0".to_string(),
            metadata: Metadata {
                name: "matrix-frame-values".to_string(),
                created: None,
                modified: None,
            },
            nodes: vec![
                Node {
                    id: "AnimatedUniform".to_string(),
                    node_type: "FloatInput".to_string(),
                    params: HashMap::from([("value".to_string(), serde_json::json!(0.0))]),
                    inputs: Vec::new(),
                    outputs: Vec::new(),
                    input_bindings: Vec::new(),
                    wgsl_override: None,
                },
                Node {
                    id: "Variation".to_string(),
                    node_type: "ResourcePool".to_string(),
                    params: HashMap::from([("selectedIndex".to_string(), serde_json::json!(3))]),
                    inputs: Vec::new(),
                    outputs: Vec::new(),
                    input_bindings: Vec::new(),
                    wgsl_override: None,
                },
            ],
            connections: Vec::new(),
            outputs: None,
            groups: Vec::new(),
            assets: Default::default(),
            state_machine: None,
            debug_artifacts: None,
        }
    }

    #[test]
    fn frame_values_update_only_exact_uniform_declarations() {
        let mut scene = frame_value_scene();
        let values = HashMap::from([(
            OverrideKey::new("AnimatedUniform", "value"),
            serde_json::json!(0.75),
        )]);

        let changed = apply_frame_uniform_values(&mut scene, &values).expect("apply frame values");

        assert_eq!(changed, 1);
        assert_eq!(scene.nodes[0].params["value"], serde_json::json!(0.75));
        assert_eq!(scene.nodes[1].params["selectedIndex"], serde_json::json!(3));
    }

    #[test]
    fn frame_values_propagate_to_all_variants_without_collapsing_them() {
        let mut first = frame_value_scene();
        let mut second = frame_value_scene();
        first.nodes[1]
            .params
            .insert("selectedIndex".to_string(), serde_json::json!(0));
        second.nodes[1]
            .params
            .insert("selectedIndex".to_string(), serde_json::json!(1));
        let values = HashMap::from([(
            OverrideKey::new("AnimatedUniform", "value"),
            serde_json::json!(0.5),
        )]);

        apply_frame_uniform_values(&mut first, &values).expect("apply first variant");
        apply_frame_uniform_values(&mut second, &values).expect("apply second variant");

        assert_eq!(first.nodes[0].params["value"], serde_json::json!(0.5));
        assert_eq!(second.nodes[0].params["value"], serde_json::json!(0.5));
        assert_eq!(first.nodes[1].params["selectedIndex"], serde_json::json!(0));
        assert_eq!(
            second.nodes[1].params["selectedIndex"],
            serde_json::json!(1)
        );
    }

    #[test]
    fn frame_values_skip_absent_variant_declarations_without_suffix_matching() {
        let mut scene = frame_value_scene();
        let values = HashMap::from([(
            OverrideKey::new("Group/AnimatedUniform", "value"),
            serde_json::json!(1.0),
        )]);

        let changed =
            apply_frame_uniform_values(&mut scene, &values).expect("project frame values");

        assert_eq!(changed, 0);
        assert_eq!(scene.nodes[0].params["value"], serde_json::json!(0.0));
    }

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
