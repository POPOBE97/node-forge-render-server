//! Reusable MTSDF glyph tiles plus a compact per-layout instance table.
//! RGB carries perpendicular distances; A carries true distance. Both are negative inside.
use super::text::{FontSource, TextFrame, TextRequest, layout_outlines};
use anyhow::{Context, Result, anyhow, ensure};
use cosmic_text::Command;
use glyphux::{
    Bitmap, Contour, Edge, FillRule, GeneratorConfig, Projection, Range, Shape, Transform, Vec2,
};
use image::{DynamicImage, Rgba, Rgba32FImage};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex, OnceLock},
};

const EM_SIZE: f32 = 64.0;
const PADDING: f64 = 16.0;
const DISTANCE_RANGE: f64 = 32.0;
const MAX_EDGE: u32 = 8192;
const MAX_PIXELS: u64 = 4 * 1024 * 1024;
const MAX_GLYPHS: usize = 8192;
const CACHE_BUDGET: usize = 32 * 1024 * 1024;

struct GlyphTile {
    image: Rgba32FImage,
    /// Unpadded outline bounds and padded tile origin in fixed-em glyph coordinates.
    bounds: [f32; 4],
    origin: [f32; 2],
}
type GlyphCache = VecDeque<([u8; 32], Arc<GlyphTile>, usize)>;
type LayoutCache = VecDeque<([u8; 32], Arc<DynamicImage>, usize)>;
static GLYPHS: OnceLock<Mutex<GlyphCache>> = OnceLock::new();
static LAYOUTS: OnceLock<Mutex<LayoutCache>> = OnceLock::new();

fn outline_shape(commands: &[Command]) -> Result<Shape> {
    let mut shape = Shape::new();
    let mut contour = Contour::new();
    let mut start = Vec2::new(0.0, 0.0);
    let mut current = start;
    let finish = |shape: &mut Shape, contour: &mut Contour, current: Vec2, start: Vec2| {
        if !contour.edges.is_empty() {
            if current != start {
                contour.add_edge(Edge::line(current, start));
            }
            shape.add_contour(std::mem::take(contour));
        }
    };
    for command in commands {
        match command {
            Command::MoveTo(p) => {
                finish(&mut shape, &mut contour, current, start);
                start = Vec2::new(f64::from(p.x), f64::from(p.y));
                current = start;
            }
            Command::LineTo(p) => {
                let end = Vec2::new(f64::from(p.x), f64::from(p.y));
                if current != end {
                    contour.add_edge(Edge::line(current, end));
                }
                current = end;
            }
            Command::QuadTo(c, p) => {
                let end = Vec2::new(f64::from(p.x), f64::from(p.y));
                contour.add_edge(Edge::quadratic(
                    current,
                    Vec2::new(f64::from(c.x), f64::from(c.y)),
                    end,
                ));
                current = end;
            }
            Command::CurveTo(c1, c2, p) => {
                let end = Vec2::new(f64::from(p.x), f64::from(p.y));
                contour.add_edge(Edge::cubic(
                    current,
                    Vec2::new(f64::from(c1.x), f64::from(c1.y)),
                    Vec2::new(f64::from(c2.x), f64::from(c2.y)),
                    end,
                ));
                current = end;
            }
            Command::Close => {
                finish(&mut shape, &mut contour, current, start);
                current = start;
            }
        }
    }
    finish(&mut shape, &mut contour, current, start);
    shape.validate().context("invalid text outline")?;
    shape.normalize();
    shape.resolve_overlaps(FillRule::NonZero);
    shape.orient_contours(FillRule::NonZero);
    Ok(shape)
}

fn glyph_tile(key: [u8; 32], commands: &[Command]) -> Result<Arc<GlyphTile>> {
    let cache = GLYPHS.get_or_init(|| Mutex::new(VecDeque::new()));
    {
        let mut cache = cache
            .lock()
            .map_err(|_| anyhow!("MTSDF glyph cache poisoned"))?;
        if let Some(i) = cache.iter().position(|entry| entry.0 == key) {
            let entry = cache.remove(i).expect("glyph cache entry");
            let tile = entry.1.clone();
            cache.push_back(entry);
            return Ok(tile);
        }
    }
    let mut shape = outline_shape(commands)?;
    let bounds = shape.bounds();
    ensure!(!bounds.is_empty(), "TextSdf glyph has an empty outline");
    let left = bounds.left.floor() - PADDING;
    let bottom = bounds.bottom.floor() - PADDING;
    let width = (bounds.right.ceil() + PADDING - left) as u32;
    let height = (bounds.top.ceil() + PADDING - bottom) as u32;
    ensure!(
        width <= MAX_EDGE
            && height <= MAX_EDGE
            && u64::from(width) * u64::from(height) <= MAX_PIXELS,
        "TextSdf glyph exceeds atlas budget"
    );
    glyphux::edge_coloring_ink_trap(&mut shape, glyphux::DEFAULT_ANGLE_THRESHOLD, 0);
    let transform = Transform::new(
        Projection::uniform(1.0, Vec2::new(-left, -bottom)),
        Range::symmetric(DISTANCE_RANGE),
    );
    let mut bitmap = Bitmap::<4>::new(width, height);
    // A single worker also works in WebAssembly without host thread support.
    let config = GeneratorConfig::default()
        .with_threads(1)
        .with_sign_correction(Some(FillRule::NonZero));
    glyphux::generate_mtsdf(&mut bitmap, &shape, &transform, &config);
    // Keep float distances unbounded; do not clamp effects to a normalized byte range.
    let values: Vec<f32> = bitmap
        .into_data()
        .into_iter()
        .map(|value| (0.5 - value) * DISTANCE_RANGE as f32)
        .collect();
    ensure!(
        values.iter().all(|value| value.is_finite()),
        "TextSdf generated non-finite glyph distances"
    );
    let image = Rgba32FImage::from_raw(width, height, values).context("invalid MTSDF bitmap")?;
    let bytes = image.as_raw().len() * size_of::<f32>();
    let tile = Arc::new(GlyphTile {
        image,
        origin: [left as f32, bottom as f32],
        bounds: [
            bounds.left as f32,
            bounds.bottom as f32,
            bounds.right as f32,
            bounds.top as f32,
        ],
    });
    if bytes <= CACHE_BUDGET {
        let mut cache = cache
            .lock()
            .map_err(|_| anyhow!("MTSDF glyph cache poisoned"))?;
        while cache.iter().map(|entry| entry.2).sum::<usize>() + bytes > CACHE_BUDGET {
            cache.pop_front();
        }
        cache.push_back((key, tile.clone(), bytes));
    }
    Ok(tile)
}

pub(crate) fn prepare(request: &TextRequest, fonts: &[FontSource]) -> Result<Arc<DynamicImage>> {
    let mut hash = Sha256::new();
    hash.update(serde_json::to_vec(request)?);
    for font in fonts {
        hash.update(font.face_index.to_le_bytes());
        hash.update(Sha256::digest(&font.bytes));
    }
    let key: [u8; 32] = hash.finalize().into();
    let cache = LAYOUTS.get_or_init(|| Mutex::new(VecDeque::new()));
    {
        let mut cache = cache
            .lock()
            .map_err(|_| anyhow!("MTSDF layout cache poisoned"))?;
        if let Some(i) = cache.iter().position(|entry| entry.0 == key) {
            let entry = cache.remove(i).expect("layout cache entry");
            let image = entry.1.clone();
            cache.push_back(entry);
            return Ok(image);
        }
    }
    let layout = layout_outlines(request, fonts, EM_SIZE)?;
    ensure!(
        layout.glyphs.len() <= MAX_GLYPHS,
        "TextSdf layout exceeds {MAX_GLYPHS} glyph instances"
    );
    let mut unique = HashMap::new();
    let mut tiles = Vec::new();
    let mut tile_area = 0_u64;
    let mut indices = Vec::new();
    let mut bounds = [0.0_f32, 0.0, layout.advance[0], layout.advance[1]];
    for glyph in &layout.glyphs {
        let index = match unique.get(&glyph.key) {
            Some(index) => *index,
            None => {
                let index = tiles.len();
                let tile = glyph_tile(glyph.key, &glyph.commands)?;
                tile_area += u64::from(tile.image.width() + 2) * u64::from(tile.image.height() + 2);
                ensure!(tile_area <= MAX_PIXELS, "TextSdf glyph atlas exceeds 4 megapixels");
                tiles.push(tile);
                unique.insert(glyph.key, index);
                index
            }
        };
        let tile = &tiles[index];
        bounds[0] = bounds[0].min(glyph.baseline[0] + tile.bounds[0] * glyph.scale);
        bounds[1] = bounds[1].min(glyph.baseline[1] - tile.bounds[3] * glyph.scale);
        bounds[2] = bounds[2].max(glyph.baseline[0] + tile.bounds[2] * glyph.scale);
        bounds[3] = bounds[3].max(glyph.baseline[1] - tile.bounds[1] * glyph.scale);
        indices.push(index);
    }
    let frame = request.measured_frame.unwrap_or_else(|| {
        if let Some(size) = request.size {
            return TextFrame {
                size,
                origin: [0.0; 2],
            };
        }
        let left = bounds[0].floor() - 1.0;
        let top = bounds[1].floor() - 1.0;
        TextFrame {
            size: [
                (bounds[2].ceil() + 1.0 - left).max(1.0),
                (bounds[3].ceil() + 1.0 - top).max(1.0),
            ],
            origin: [-left, -top],
        }
    });
    ensure!(
        frame
            .size
            .iter()
            .all(|value| value.is_finite() && *value > 0.0),
        "TextSdf layout size must be positive and finite"
    );
    let widest = tiles
        .iter()
        .map(|tile| tile.image.width() + 2)
        .max()
        .unwrap_or(4);
    let width = widest
        .max((tile_area as f64).sqrt().ceil() as u32)
        .max(4)
        .next_power_of_two()
        .min(MAX_EDGE);
    ensure!(widest <= width, "TextSdf glyph exceeds atlas width");
    // Header and two texels per glyph instance precede the packed distance tiles.
    let metadata_rows = (1 + 2 * layout.glyphs.len() as u32).div_ceil(width);
    let (mut x, mut y, mut row_height) = (1, metadata_rows + 1, 0);
    let mut locations = Vec::new();
    for tile in &tiles {
        if x + tile.image.width() + 1 > width {
            x = 1;
            y += row_height + 2;
            row_height = 0;
        }
        locations.push([x, y]);
        row_height = row_height.max(tile.image.height());
        x += tile.image.width() + 2;
    }
    let height = y + row_height + 1;
    ensure!(
        height <= MAX_EDGE && u64::from(width) * u64::from(height) <= MAX_PIXELS,
        "TextSdf glyph atlas exceeds 4 megapixels or the 8192 pixel edge limit"
    );
    let mut atlas = Rgba32FImage::new(width, height);
    atlas.put_pixel(
        0,
        0,
        Rgba([
            layout.glyphs.len() as f32,
            frame.size[0],
            frame.size[1],
            metadata_rows as f32,
        ]),
    );
    for (i, (glyph, &tile_index)) in layout.glyphs.iter().zip(&indices).enumerate() {
        let tile = &tiles[tile_index];
        let left = glyph.baseline[0] + frame.origin[0] + tile.origin[0] * glyph.scale;
        let bottom =
            frame.size[1] - glyph.baseline[1] - frame.origin[1] + tile.origin[1] * glyph.scale;
        let record = [
            [
                left,
                bottom,
                left + tile.image.width() as f32 * glyph.scale,
                bottom + tile.image.height() as f32 * glyph.scale,
            ],
            [
                locations[tile_index][0] as f32,
                locations[tile_index][1] as f32,
                tile.image.width() as f32,
                tile.image.height() as f32,
            ],
        ];
        for (j, value) in record.into_iter().enumerate() {
            let index = 1 + 2 * i as u32 + j as u32;
            atlas.put_pixel(index % width, index / width, Rgba(value));
        }
    }
    for (tile, [x, y]) in tiles.iter().zip(locations) {
        for row in 0..tile.image.height() {
            let start = ((y + row) * width + x) as usize * 4;
            let source = row as usize * tile.image.width() as usize * 4;
            let len = tile.image.width() as usize * 4;
            atlas.as_mut()[start..start + len]
                .copy_from_slice(&tile.image.as_raw()[source..source + len]);
        }
    }
    let image = Arc::new(DynamicImage::ImageRgba32F(atlas));
    let bytes = image.as_bytes().len();
    if bytes <= CACHE_BUDGET {
        let mut cache = cache
            .lock()
            .map_err(|_| anyhow!("MTSDF layout cache poisoned"))?;
        while cache.iter().map(|entry| entry.2).sum::<usize>() + bytes > CACHE_BUDGET {
            cache.pop_front();
        }
        cache.push_back((key, image.clone(), bytes));
    }
    Ok(image)
}

#[cfg(test)]
#[path = "text_sdf_tests.rs"]
mod tests;
