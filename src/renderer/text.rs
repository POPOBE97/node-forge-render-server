//! CPU text shaping and native-density glyph atlas preparation. No system font discovery.
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{Arc, Mutex, OnceLock},
};

use anyhow::{Context, Result, anyhow, bail, ensure};
use cosmic_text::{
    Align, Attrs, Buffer, CacheKey, CacheKeyFlags, Fallback, Family, FontSystem, Metrics, Shaping,
    SwashCache, SwashContent, Wrap, fontdb,
};
use image::{DynamicImage, Rgba, RgbaImage};
use serde::Serialize;
use sha2::{Digest, Sha256};

const FAMILIES: [&str; 2] = ["NodeForge.Primary", "NodeForge.Fallback"];
const CACHE_BYTES: usize = 32 * 1024 * 1024;

struct ArchiveFallback;
impl Fallback for ArchiveFallback {
    fn common_fallback(&self) -> &[&'static str] {
        &FAMILIES[1..]
    }
    fn forbidden_fallback(&self) -> &[&'static str] {
        &[]
    }
    fn script_fallback(&self, _: unicode_script::Script, _: &str) -> &[&'static str] {
        &[]
    }
}

pub(crate) struct FontSource {
    pub bytes: Vec<u8>,
    pub face_index: u32,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub(crate) struct TextFrame {
    pub size: [f32; 2],
    pub origin: [f32; 2],
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct TextRequest {
    pub text: String,
    pub font_size: f32,
    pub weight: u16,
    pub line_height: f32,
    pub letter_spacing: f32,
    pub size: Option<[f32; 2]>,
    pub align: String,
    pub vertical_align: String,
    pub wrap: String,
    pub hinting: bool,
    pub density: f32,
    /// Raster phase of the layout's top-left, in physical pixels (renderer-private).
    pub phase: [f32; 2],
    pub measured_frame: Option<TextFrame>,
}

pub(crate) struct TextMesh {
    /// Centered LocalPx vertices: position.xyz and private atlas uv.xy.
    pub vertices: Arc<[u8]>,
    pub atlas: Arc<DynamicImage>,
    pub size: [f32; 2],
    pub frame: TextFrame,
    #[cfg(test)]
    pub glyph_count: usize,
}

type MeshCache = VecDeque<([u8; 32], Arc<TextMesh>, usize)>;
static CACHE: OnceLock<Mutex<MeshCache>> = OnceLock::new();

pub(crate) fn prepare_text(
    request: &TextRequest,
    fonts: &[FontSource],
    max_atlas: u32,
) -> Result<Arc<TextMesh>> {
    ensure!(
        !fonts.is_empty() && fonts.len() <= FAMILIES.len(),
        "TextPass needs a primary font and at most one fallback font"
    );
    ensure!(
        request.text.len() <= 256 * 1024,
        "TextPass text exceeds 256 KiB"
    );
    ensure!(
        request.font_size.is_finite() && request.font_size > 0.0 && request.font_size <= 4096.0,
        "TextPass fontSize must be in (0, 4096]"
    );
    ensure!(
        request.density.is_finite()
            && request.density > 0.0
            && request.density * request.font_size <= 4096.0,
        "TextPass pixel density must be positive and physical font size must not exceed 4096 px"
    );
    ensure!(
        request.line_height.is_finite() && request.line_height > 0.0,
        "TextPass lineHeight must be positive"
    );
    ensure!(
        request.letter_spacing.is_finite(),
        "TextPass letterSpacing must be finite"
    );
    if let Some(size) = request.size {
        ensure!(
            size.iter().all(|n| n.is_finite() && *n > 0.0),
            "TextPass size must be positive and finite"
        );
    }
    let mut hash = Sha256::new();
    hash.update(serde_json::to_vec(request)?);
    hash.update(max_atlas.to_le_bytes());
    for font in fonts {
        hash.update(font.face_index.to_le_bytes());
        hash.update(Sha256::digest(&font.bytes));
    }
    let key: [u8; 32] = hash.finalize().into();
    let cache = CACHE.get_or_init(|| Mutex::new(VecDeque::new()));
    {
        let mut cache = cache.lock().map_err(|_| anyhow!("text cache poisoned"))?;
        if let Some(i) = cache.iter().position(|entry| entry.0 == key) {
            let entry = cache.remove(i).expect("cache entry exists");
            let mesh = entry.1.clone();
            cache.push_back(entry);
            return Ok(mesh);
        }
    }
    let mesh = Arc::new(build_text(request, fonts, max_atlas)?);
    let bytes = mesh.vertices.len() + mesh.atlas.as_bytes().len();
    if bytes <= CACHE_BYTES {
        let mut cache = cache.lock().map_err(|_| anyhow!("text cache poisoned"))?;
        while cache.iter().map(|entry| entry.2).sum::<usize>() + bytes > CACHE_BYTES {
            cache.pop_front();
        }
        cache.push_back((key, mesh.clone(), bytes));
    }
    Ok(mesh)
}

fn font_system(fonts: &[FontSource]) -> Result<FontSystem> {
    let mut db = fontdb::Database::new();
    for (index, font) in fonts.iter().enumerate() {
        let loaded = db.load_font_source(fontdb::Source::Binary(Arc::new(font.bytes.clone())));
        let mut selected = loaded
            .iter()
            .filter_map(|id| db.face(*id))
            .find(|face| face.index == font.face_index)
            .cloned()
            .with_context(|| {
                format!(
                    "invalid font or missing faceIndex {} in font {}",
                    font.face_index, index
                )
            })?;
        for id in loaded {
            db.remove_face(id);
        }
        // Stable private aliases enforce the authored fallback order on every OS.
        selected.families = vec![(
            FAMILIES[index].into(),
            fontdb::Language::English_UnitedStates,
        )];
        db.push_face_info(selected);
    }
    Ok(FontSystem::new_with_locale_and_db_and_fallback(
        "en-US".into(),
        db,
        ArchiveFallback,
    ))
}

struct GlyphQuad {
    key: CacheKey,
    left: f32,
    top: f32,
    width: u32,
    height: u32,
}

fn build_text(req: &TextRequest, fonts: &[FontSource], max_atlas: u32) -> Result<TextMesh> {
    let mut fs = font_system(fonts)?;
    let mut buffer = Buffer::new(&mut fs, Metrics::new(req.font_size, req.line_height));
    let align = match req.align.as_str() {
        "left" => Align::Left,
        "center" => Align::Center,
        "right" => Align::Right,
        _ => bail!("invalid TextPass align"),
    };
    buffer.set_wrap(match req.wrap.as_str() {
        "none" => Wrap::None,
        "word" => Wrap::WordOrGlyph,
        "glyph" => Wrap::Glyph,
        _ => bail!("invalid TextPass wrap"),
    });
    buffer.set_size(req.size.map(|size| size[0]), None);
    let flags = if req.hinting {
        CacheKeyFlags::empty()
    } else {
        CacheKeyFlags::DISABLE_HINTING
    };
    let primary_weight = fs
        .db()
        .faces()
        .next()
        .context("empty font database")?
        .weight;
    let attrs = Attrs::new()
        .family(Family::Name(FAMILIES[0]))
        .weight(if req.weight == 0 {
            primary_weight
        } else {
            fontdb::Weight(req.weight)
        })
        .letter_spacing(req.letter_spacing / req.font_size)
        .cache_key_flags(flags);
    buffer.set_text(&req.text, &attrs, Shaping::Advanced, Some(align));
    buffer.shape_until_scroll(&mut fs, false);
    let advance_width = buffer
        .layout_runs()
        .map(|run| run.line_w)
        .fold(0.0_f32, f32::max);
    let advance_height = buffer
        .layout_runs()
        .map(|run| run.line_top + run.line_height)
        .fold(req.line_height, f32::max);
    let provisional_size = req.size.unwrap_or([advance_width.max(1.0), advance_height]);
    let v_offset = match req.vertical_align.as_str() {
        "top" => 0.0,
        "center" => (provisional_size[1] - advance_height) * 0.5,
        "bottom" => provisional_size[1] - advance_height,
        _ => bail!("invalid TextPass verticalAlign"),
    };
    let mut swash = SwashCache::new();
    let mut checked_fonts = HashSet::new();
    let mut quads = Vec::new();
    let mut glyphs = HashMap::new();
    let mut bounds = [0.0_f32, 0.0_f32, advance_width, advance_height];
    for run in buffer.layout_runs() {
        for glyph in run.glyphs {
            ensure!(
                glyph.glyph_id != 0,
                "missing glyph for {:?}; embed a fallback font",
                &run.text[glyph.start..glyph.end]
            );
            if req.weight != 0 && checked_fonts.insert(glyph.font_id) {
                let face_weight = fs
                    .db()
                    .face(glyph.font_id)
                    .context("missing shaped font face")?
                    .weight
                    .0;
                let font = fs
                    .get_font(glyph.font_id, glyph.font_weight)
                    .context("cannot load shaped font")?;
                if let Some(axis) = font
                    .as_swash()
                    .variations()
                    .find_by_tag(u32::from_be_bytes(*b"wght"))
                {
                    ensure!(
                        (axis.min_value()..=axis.max_value()).contains(&(req.weight as f32)),
                        "TextPass fontWeight {} is outside embedded font's wght range",
                        req.weight
                    );
                } else {
                    ensure!(
                        req.weight == face_weight,
                        "TextPass fontWeight {} requires a matching font face (embedded face weight is {})",
                        req.weight,
                        face_weight
                    );
                }
            }
            // CacheKey::new preserves fractional Y; LayoutGlyph::physical deliberately truncates it.
            let x = (glyph.x + glyph.font_size * glyph.x_offset) * req.density + req.phase[0];
            let y = (run.line_y + v_offset + glyph.y - glyph.font_size * glyph.y_offset)
                * req.density
                + req.phase[1];
            let (key, px, py) = CacheKey::new(
                glyph.font_id,
                glyph.glyph_id,
                glyph.font_size * req.density,
                (x, y),
                glyph.font_weight,
                glyph.cache_key_flags,
            );
            let Some(image) = swash.get_image(&mut fs, key).as_ref() else {
                continue;
            };
            ensure!(
                image.content == SwashContent::Mask,
                "TextPass currently requires monochrome outline fonts"
            );
            let p = image.placement;
            if p.width == 0 || p.height == 0 {
                continue;
            }
            let left = (px as f32 + p.left as f32 - req.phase[0]) / req.density;
            let top = (py as f32 - p.top as f32 - req.phase[1]) / req.density;
            bounds[0] = bounds[0].min(left);
            bounds[1] = bounds[1].min(top);
            bounds[2] = bounds[2].max(left + p.width as f32 / req.density);
            bounds[3] = bounds[3].max(top + p.height as f32 / req.density);
            glyphs.entry(key).or_insert_with(|| image.clone());
            quads.push(GlyphQuad {
                key,
                left,
                top,
                width: p.width,
                height: p.height,
            });
        }
    }
    // Keep an integer measurement frame independent of the final raster phase.
    // One pixel of padding preserves overhangs at fractional origins.
    let frame = req.measured_frame.unwrap_or_else(|| {
        if let Some(size) = req.size {
            TextFrame {
                size,
                origin: [0.0; 2],
            }
        } else {
            let left = bounds[0].floor() - 1.0;
            let top = bounds[1].floor() - 1.0;
            TextFrame {
                size: [
                    (bounds[2].ceil() + 1.0 - left).max(1.0),
                    (bounds[3].ceil() + 1.0 - top).max(1.0),
                ],
                origin: [-left, -top],
            }
        }
    });
    let size = frame.size;
    let auto_shift = frame.origin;
    // Shelf packing in glyph encounter order (HashMap iteration must not affect reproducibility).
    let atlas_width = max_atlas.min(2048);
    ensure!(
        atlas_width >= 4,
        "device texture limit too small for TextPass"
    );
    let mut locations = HashMap::new();
    let (mut ax, mut ay, mut row_height) = (1_u32, 1_u32, 0_u32);
    for quad in &quads {
        if locations.contains_key(&quad.key) {
            continue;
        }
        ensure!(
            quad.width + 2 <= atlas_width,
            "TextPass glyph exceeds atlas width {atlas_width}"
        );
        if ax + quad.width + 1 > atlas_width {
            ax = 1;
            ay += row_height + 2;
            row_height = 0;
        }
        ensure!(
            ay + quad.height + 1 <= max_atlas,
            "TextPass glyph atlas exceeds device limit {max_atlas}"
        );
        locations.insert(quad.key, [ax, ay]);
        row_height = row_height.max(quad.height);
        ax += quad.width + 2;
    }
    let atlas_height = (ay + row_height + 1).max(2);
    ensure!(
        atlas_width as usize * atlas_height as usize <= 16 * 1024 * 1024,
        "TextPass atlas exceeds 16 megapixels"
    );
    let mut atlas = RgbaImage::new(atlas_width, atlas_height);
    for (key, [x, y]) in &locations {
        let glyph = &glyphs[key];
        for gy in 0..glyph.placement.height {
            for gx in 0..glyph.placement.width {
                let coverage = glyph.data[(gy * glyph.placement.width + gx) as usize];
                atlas.put_pixel(x + gx, y + gy, Rgba([255, 255, 255, coverage]));
            }
        }
    }
    let mut vertices = Vec::<f32>::new();
    for quad in &quads {
        let [ax, ay] = locations[&quad.key];
        let left = quad.left + auto_shift[0] - size[0] * 0.5;
        let right = left + quad.width as f32 / req.density;
        let top = size[1] * 0.5 - quad.top - auto_shift[1];
        let bottom = top - quad.height as f32 / req.density;
        let uv = [
            ax as f32 / atlas_width as f32,
            ay as f32 / atlas_height as f32,
            (ax + quad.width) as f32 / atlas_width as f32,
            (ay + quad.height) as f32 / atlas_height as f32,
        ];
        for v in [
            [left, bottom, 0.0, uv[0], uv[3]],
            [right, bottom, 0.0, uv[2], uv[3]],
            [right, top, 0.0, uv[2], uv[1]],
            [left, bottom, 0.0, uv[0], uv[3]],
            [right, top, 0.0, uv[2], uv[1]],
            [left, top, 0.0, uv[0], uv[1]],
        ] {
            vertices.extend(v);
        }
    }
    // A degenerate triangle keeps empty/whitespace-only text a valid transparent draw.
    if vertices.is_empty() {
        vertices.resize(15, 0.0);
    }
    Ok(TextMesh {
        vertices: Arc::from(bytemuck::cast_slice::<f32, u8>(&vertices)),
        atlas: Arc::new(DynamicImage::ImageRgba8(atlas)),
        size,
        frame,
        #[cfg(test)]
        glyph_count: quads.len(),
    })
}

pub(crate) fn format_numbers(format: &str, values: &[f64], decimals: usize) -> Result<String> {
    ensure!(
        decimals <= 6,
        "StringFormat decimals must be between 0 and 6"
    );
    let mut chars = format.chars().peekable();
    let mut out = String::new();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                out.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                out.push('}');
            }
            '{' => {
                let mut index = String::new();
                let mut closed = false;
                for next in chars.by_ref() {
                    if next == '}' {
                        closed = true;
                        break;
                    }
                    index.push(next);
                }
                ensure!(closed, "unclosed StringFormat placeholder");
                let index: usize = index
                    .parse()
                    .context("StringFormat placeholder must be a numeric index")?;
                let value = values
                    .get(index)
                    .with_context(|| format!("StringFormat has no value{index}"))?;
                ensure!(value.is_finite(), "StringFormat requires finite numbers");
                out.push_str(&format!("{value:.decimals$}"));
            }
            '}' => bail!("unescaped closing brace in StringFormat"),
            _ => out.push(c),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> TextRequest {
        TextRequest {
            text: "AV fi\ngyp".into(),
            font_size: 23.3333,
            weight: 0,
            line_height: 30.0,
            letter_spacing: 0.0,
            size: None,
            align: "left".into(),
            vertical_align: "top".into(),
            wrap: "none".into(),
            hinting: false,
            density: 1.0,
            phase: [0.0, 0.0],
            measured_frame: None,
        }
    }
    fn fonts() -> Vec<FontSource> {
        vec![FontSource {
            bytes: include_bytes!("../../assets/fonts/GeistMono/GeistMono-Regular.ttf").to_vec(),
            face_index: 0,
        }]
    }
    #[test]
    fn formats_dynamic_dimensions_and_rejects_bad_placeholders() {
        assert_eq!(
            format_numbers("{{{0} × {1}}}", &[1260.0, 2800.0], 0).unwrap(),
            "{1260 × 2800}"
        );
        assert!(format_numbers("{2}", &[1.0], 0).is_err());
        assert!(format_numbers("{0", &[1.0], 0).is_err());
    }
    #[test]
    fn rasterizes_fractional_size_and_reuses_cached_mesh() {
        let req = request();
        let a = prepare_text(&req, &fonts(), 4096).unwrap();
        assert!(a.glyph_count > 0 && a.size[1] >= 60.0);
        assert!(
            a.atlas
                .as_rgba8()
                .unwrap()
                .pixels()
                .any(|p| p[3] > 0 && p[3] < 255)
        );
        let b = prepare_text(&req, &fonts(), 4096).unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        let mut changed = req.clone();
        changed.density = 2.0;
        let c = prepare_text(&changed, &fonts(), 4096).unwrap();
        assert!(!Arc::ptr_eq(&a, &c));
        assert!((a.size[0] - c.size[0]).abs() <= 2.0);
    }
    #[test]
    fn explicit_fallback_handles_chinese_and_missing_font_is_an_error() {
        let mut req = request();
        req.text = "中文测试 AV".into();
        assert!(prepare_text(&req, &fonts(), 4096).is_err());
        let mut source = fonts();
        source.push(FontSource {
            bytes: include_bytes!("../../assets/fonts/MiSans/MiSans-Regular.ttf").to_vec(),
            face_index: 0,
        });
        let mesh = prepare_text(&req, &source, 4096).unwrap();
        assert!(mesh.glyph_count >= 6);
    }
    #[test]
    fn wrapping_changes_layout_without_scaling_font() {
        let mut req = request();
        req.text = "one two three four five six".into();
        req.size = Some([100.0, 300.0]);
        req.wrap = "word".into();
        let mesh = prepare_text(&req, &fonts(), 4096).unwrap();
        assert_eq!(mesh.size, [100.0, 300.0]);
        let v: &[f32] = bytemuck::cast_slice(&mesh.vertices);
        let ys: Vec<_> = v.chunks(5).map(|v| v[1]).collect();
        assert!(ys.iter().copied().fold(f32::MAX, f32::min) < 70.0);
    }

    #[test]
    fn empty_text_auto_alignment_and_invalid_faces_are_deterministic() {
        let mut req = request();
        req.text.clear();
        let empty = prepare_text(&req, &fonts(), 4096).unwrap();
        assert_eq!(empty.glyph_count, 0);
        assert!(empty.atlas.as_rgba8().unwrap().pixels().all(|p| p[3] == 0));
        req.text = "gyp AV".into();
        for align in ["left", "center", "right"] {
            req.align = align.into();
            let mesh = prepare_text(&req, &fonts(), 4096).unwrap();
            assert!(mesh.size.iter().all(|n| n.is_finite() && *n < 200.0));
        }
        let mut invalid = fonts();
        invalid[0].face_index = 100;
        assert!(prepare_text(&req, &invalid, 4096).is_err());
        req.weight = 999;
        assert!(prepare_text(&req, &fonts(), 4096).is_err());
    }

    #[test]
    fn auto_frame_preserves_overhangs_at_fractional_origins() {
        let mut req = request();
        req.text = "jÁgyp".into();
        let measured = prepare_text(&req, &fonts(), 4096).unwrap();
        req.measured_frame = Some(measured.frame);
        req.phase = [0.25, 0.75];
        let mesh = prepare_text(&req, &fonts(), 4096).unwrap();
        assert_eq!(mesh.size, measured.size);
        let vertices: &[f32] = bytemuck::cast_slice(&mesh.vertices);
        for vertex in vertices.chunks_exact(5) {
            assert!(vertex[0].abs() <= mesh.size[0] * 0.5);
            assert!(vertex[1].abs() <= mesh.size[1] * 0.5);
        }
    }
}
