use std::sync::Arc;

use anyhow::Result;
use rust_wgpu_fiber::{
    eframe::{egui, egui_wgpu, wgpu},
    shader_space::ShaderSpace,
    ResourceName,
};

use crate::{asset_store::AssetStore, dsl::SceneDSL, renderer};

use super::types::{MatrixConfig, ResourcePoolInfo};

#[derive(Clone, Debug)]
pub struct MatrixCellCoord {
    pub row: usize,
    pub col: usize,
}

pub struct MatrixCell {
    pub coord: MatrixCellCoord,
    pub label: String,
    pub shader_space: ShaderSpace,
    pub output_texture_name: ResourceName,
    pub egui_texture_id: Option<egui::TextureId>,
}

pub struct MatrixRenderState {
    pub cells: Vec<MatrixCell>,
    pub grid_rows: usize,
    pub grid_cols: usize,
    pub cell_resolution: [u32; 2],
    pub row_pool_id: Option<String>,
    pub col_pool_id: Option<String>,
    pub base_pipeline_signature: Option<[u8; 32]>,
}

impl Default for MatrixRenderState {
    fn default() -> Self {
        Self {
            cells: Vec::new(),
            grid_rows: 0,
            grid_cols: 0,
            cell_resolution: [0, 0],
            row_pool_id: None,
            col_pool_id: None,
            base_pipeline_signature: None,
        }
    }
}

impl MatrixRenderState {
    pub fn clear(&mut self, renderer: &mut egui_wgpu::Renderer) {
        for cell in self.cells.drain(..) {
            if let Some(id) = cell.egui_texture_id {
                renderer.free_texture(&id);
            }
        }
        self.grid_rows = 0;
        self.grid_cols = 0;
        self.cell_resolution = [0, 0];
        self.row_pool_id = None;
        self.col_pool_id = None;
        self.base_pipeline_signature = None;
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

    let (grid_rows, grid_cols, row_pool, col_pool) = match selected_pools.len() {
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

    state.grid_rows = grid_rows;
    state.grid_cols = grid_cols;
    state.row_pool_id = row_pool.map(|p| p.node_id.clone());
    state.col_pool_id = col_pool.map(|p| p.node_id.clone());

    for row in 0..grid_rows {
        for col in 0..grid_cols {
            let mut variant_scene = params.scene.clone();

            if let Some(rp) = row_pool {
                patch_pool_index(&mut variant_scene, &rp.node_id, row as i64);
            }
            if let Some(cp) = col_pool {
                patch_pool_index(&mut variant_scene, &cp.node_id, col as i64);
            }

            let label = match (row_pool, col_pool) {
                (Some(_), Some(_)) => format!("[{}, {}]", row, col),
                (None, Some(_)) => format!("[{}]", col),
                _ => format!("[{}, {}]", row, col),
            };

            let mut builder = renderer::ShaderSpaceBuilder::new(
                params.device.clone(),
                params.queue.clone(),
            )
            .with_options(renderer::ShaderSpaceBuildOptions {
                presentation_mode: renderer::ShaderSpacePresentationMode::UiHdrNative,
                ..Default::default()
            })
            .with_asset_store(params.asset_store.clone());
            if let Some(adapter) = params.adapter {
                builder = builder.with_adapter(adapter.clone());
            }
            let build_result = builder
            .build(&variant_scene);

            match build_result {
                Ok(result) => {
                    result.shader_space.render();

                    let tex_name = result.present_output_texture.clone();
                    let egui_id = if let Some(tex) =
                        result.shader_space.textures.get(tex_name.as_str())
                    {
                        if state.cell_resolution == [0, 0] {
                            if let Some(info) = result.shader_space.texture_info(tex_name.as_str()) {
                                state.cell_resolution = [info.size.width, info.size.height];
                            }
                        }
                        tex.wgpu_texture_view.as_ref().map(|view| {
                            renderer.register_native_texture_with_sampler_options(
                                &render_state.device,
                                view,
                                super::texture_bridge::canvas_sampler_descriptor(
                                    wgpu::FilterMode::Linear,
                                ),
                            )
                        })
                    } else {
                        None
                    };

                    state.cells.push(MatrixCell {
                        coord: MatrixCellCoord { row, col },
                        label,
                        shader_space: result.shader_space,
                        output_texture_name: tex_name,
                        egui_texture_id: egui_id,
                    });
                }
                Err(e) => {
                    eprintln!("[matrix] failed to build cell [{row}, {col}]: {e:#}");
                }
            }
        }
    }

    Ok(())
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
