//! Public geometry is bottom-left/Y-up. One unit is an adjacent R/B center pitch.
use serde::{Deserialize, Serialize};

use super::calibration::Calibration;

pub type Point = [f64; 2];
pub fn add(a: Point, b: Point) -> Point {
    [a[0] + b[0], a[1] + b[1]]
}
pub fn sub(a: Point, b: Point) -> Point {
    [a[0] - b[0], a[1] - b[1]]
}
pub fn mul(a: Point, s: f64) -> Point {
    [a[0] * s, a[1] * s]
}
pub fn length(a: Point) -> f64 {
    a[0].hypot(a[1])
}

pub fn cell_to_world(cell: Point) -> Point {
    [cell[0] - cell[1], cell[0] + cell[1]]
}
pub fn world_to_cell(p: Point) -> Point {
    [(p[0] + p[1]) * 0.5, (p[1] - p[0]) * 0.5]
}
pub fn snapped_delta(delta: Point) -> [i32; 2] {
    let cell = world_to_cell(delta);
    [cell[0].round() as i32, cell[1].round() as i32]
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Subpixel {
    #[default]
    Red,
    Blue,
    Green1,
    Green2,
}
impl Subpixel {
    pub const ALL: [Self; 4] = [Self::Red, Self::Blue, Self::Green1, Self::Green2];
    pub fn label(self) -> &'static str {
        match self {
            Self::Red => "R",
            Self::Blue => "B",
            Self::Green1 => "G1 /",
            Self::Green2 => "G2 \\",
        }
    }
    pub fn offset(self) -> Point {
        match self {
            Self::Red => [0., 0.],
            Self::Blue => [0.5, 0.5],
            Self::Green1 => [0.5, 0.],
            Self::Green2 => [0., 0.5],
        }
    }
    pub fn color(self) -> [f32; 3] {
        match self {
            Self::Red => [1., 0.08, 0.12],
            Self::Blue => [0.12, 0.4, 1.],
            _ => [0.15, 1., 0.4],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GridSite {
    pub cell: [i32; 2],
    pub kind: Subpixel,
}
impl GridSite {
    pub fn world(self) -> Point {
        cell_to_world(add(
            [self.cell[0] as f64, self.cell[1] as f64],
            self.kind.offset(),
        ))
    }
    pub fn nearest(p: Point, kind: Subpixel) -> Self {
        let cell = sub(world_to_cell(p), kind.offset());
        Self {
            cell: [cell[0].round() as i32, cell[1].round() as i32],
            kind,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Landmark {
    pub source: Point,
    pub target: GridSite,
    pub enabled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Measurement {
    pub a: Point,
    pub b: Point,
    /// Relative color-signal threshold for image line-scan peaks; independent of grid spacing.
    pub threshold: f64,
}
impl Measurement {
    pub fn new(a: Point, b: Point) -> Self {
        Self {
            a,
            b,
            threshold: 0.12,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GuideAxis {
    Horizontal,
    Vertical,
}
impl GuideAxis {
    pub fn coordinate(self) -> usize {
        if self == Self::Vertical { 0 } else { 1 }
    }
    pub fn label(self) -> &'static str {
        if self == Self::Vertical { "X" } else { "Y" }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Guide {
    pub id: u64,
    pub axis: GuideAxis,
    /// Shared corrected-grid coordinate, independent of any image or viewport.
    pub position: f64,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RulerSettings {
    pub visible: bool,
    pub guides_visible: bool,
    pub locked: bool,
    pub guides: Vec<Guide>,
}
impl Default for RulerSettings {
    fn default() -> Self {
        Self {
            visible: true,
            guides_visible: true,
            locked: false,
            guides: vec![],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GridSettings {
    pub visible: bool,
    pub lines: bool,
    pub centers: bool,
    pub outlines: bool,
    pub labels: bool,
    pub opacity: f32,
    pub min_spacing: f32,
}
impl Default for GridSettings {
    fn default() -> Self {
        Self {
            visible: true,
            lines: true,
            centers: true,
            outlines: false,
            labels: true,
            opacity: 0.45,
            min_spacing: 16.,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Viewport {
    pub center: Point,
    pub zoom: f64,
}
impl Default for Viewport {
    fn default() -> Self {
        Self {
            center: [0., 0.],
            zoom: 24.,
        }
    }
}
impl Viewport {
    /// UI coordinates enter the Y-up domain exactly here.
    pub fn screen_to_world(&self, p: Point, size: Point) -> Point {
        [
            self.center[0] + (p[0] - size[0] * 0.5) / self.zoom,
            self.center[1] - (p[1] - size[1] * 0.5) / self.zoom,
        ]
    }
    pub fn world_to_screen(&self, p: Point, size: Point) -> Point {
        [
            (p[0] - self.center[0]) * self.zoom + size[0] * 0.5,
            size[1] * 0.5 - (p[1] - self.center[1]) * self.zoom,
        ]
    }
    pub fn zoom_at(&mut self, factor: f64, p: Point, size: Point) {
        let before = self.screen_to_world(p, size);
        self.zoom = (self.zoom * factor).clamp(0.02, 4096.);
        self.center = add(self.center, sub(before, self.screen_to_world(p, size)));
    }
    pub fn bounds(&self, size: Point) -> [f64; 4] {
        let half = mul(size, 0.5 / self.zoom);
        [
            self.center[0] - half[0],
            self.center[1] - half[1],
            self.center[0] + half[0],
            self.center[1] + half[1],
        ]
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImageLayer {
    pub id: u64,
    pub asset: String,
    pub name: String,
    pub size: [u32; 2],
    pub bit_depth: u8,
    pub visible: bool,
    pub locked: bool,
    pub opacity: f32,
    pub offset: [i32; 2],
    pub calibration: Calibration,
    pub landmarks: Vec<Landmark>,
}
impl ImageLayer {
    pub fn rotate_image(&mut self, degrees: f64, world_pivot: Point) -> anyhow::Result<()> {
        anyhow::ensure!(!self.locked, "Unlock the image before rotating it");
        self.calibration = self.calibration.rotated_about(
            degrees,
            sub(world_pivot, self.offset_world()),
            self.size,
        )?;
        Ok(())
    }
    pub fn offset_world(&self) -> Point {
        cell_to_world([self.offset[0] as f64, self.offset[1] as f64])
    }
    pub fn sample_position(&self, world: Point) -> Option<Point> {
        self.calibration
            .map(sub(world, self.offset_world()), self.size)
    }
    pub fn source_to_world(&self, source: Point) -> Option<Point> {
        self.calibration
            .unmap(source, self.size)
            .map(|p| add(p, self.offset_world()))
    }
    pub fn contains(&self, world: Point) -> bool {
        self.sample_position(world).is_some_and(|p| {
            p[0] >= 0. && p[1] >= 0. && p[0] < self.size[0] as f64 && p[1] < self.size[1] as f64
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompareMode {
    #[default]
    Overlay,
    Difference,
    Ab,
    Wipe,
}
impl CompareMode {
    pub const ALL: [Self; 4] = [Self::Overlay, Self::Difference, Self::Ab, Self::Wipe];
    pub fn label(self) -> &'static str {
        match self {
            Self::Overlay => "Overlay",
            Self::Difference => "Difference",
            Self::Ab => "A / B",
            Self::Wipe => "Wipe",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Comparison {
    pub mode: CompareMode,
    pub a: Option<u64>,
    pub b: Option<u64>,
    pub show_b: bool,
    pub wipe: f32,
    pub horizontal: bool,
    pub gain: f32,
}
impl Default for Comparison {
    fn default() -> Self {
        Self {
            mode: CompareMode::Overlay,
            a: None,
            b: None,
            show_b: false,
            wipe: 0.5,
            horizontal: false,
            gain: 1.,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExportSettings {
    pub bounds: [f64; 4],
    pub pixels_per_unit: f64,
    pub bit_depth: u8,
    pub selected_only: bool,
    pub grid: bool,
}
impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            bounds: [-10., -10., 10., 10.],
            pixels_per_unit: 32.,
            bit_depth: 8,
            selected_only: false,
            grid: false,
        }
    }
}
impl ExportSettings {
    pub fn dimensions(&self) -> anyhow::Result<[u32; 2]> {
        anyhow::ensure!(
            self.bounds.iter().all(|v| v.is_finite())
                && self.pixels_per_unit.is_finite()
                && self.pixels_per_unit > 0.,
            "Invalid export coordinates or density"
        );
        let w = ((self.bounds[2] - self.bounds[0]) * self.pixels_per_unit).ceil();
        let h = ((self.bounds[3] - self.bounds[1]) * self.pixels_per_unit).ceil();
        anyhow::ensure!(
            w >= 1. && h >= 1. && w <= 100_000. && h <= 100_000. && w * h <= 1_000_000_000.,
            "Export must be 1–100,000 pixels per side and at most 1 gigapixel"
        );
        anyhow::ensure!(
            matches!(self.bit_depth, 8 | 16),
            "PNG bit depth must be 8 or 16"
        );
        Ok([w as u32, h as u32])
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PixelPerfectoDocument {
    pub version: u32,
    pub grid: GridSettings,
    pub layers: Vec<ImageLayer>,
    pub selected: Option<u64>,
    pub viewport: Viewport,
    pub comparison: Comparison,
    pub nearest: bool,
    pub export: ExportSettings,
    /// Optional workspace annotations in the version-1 document; absent means no authored guides.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rulers: Option<RulerSettings>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub measurement: Option<Measurement>,
}
impl Default for PixelPerfectoDocument {
    fn default() -> Self {
        Self {
            version: 1,
            grid: GridSettings::default(),
            layers: vec![],
            selected: None,
            viewport: Viewport::default(),
            comparison: Comparison::default(),
            nearest: false,
            export: ExportSettings::default(),
            rulers: None,
            measurement: None,
        }
    }
}
impl PixelPerfectoDocument {
    pub fn rulers(&self) -> &RulerSettings {
        static EMPTY: RulerSettings = RulerSettings {
            visible: true,
            guides_visible: true,
            locked: false,
            guides: Vec::new(),
        };
        self.rulers.as_ref().unwrap_or(&EMPTY)
    }
    pub fn rulers_mut(&mut self) -> &mut RulerSettings {
        self.rulers.get_or_insert_with(RulerSettings::default)
    }
    pub fn selected_layer(&self) -> Option<&ImageLayer> {
        self.layers.iter().find(|l| Some(l.id) == self.selected)
    }
    pub fn selected_layer_mut(&mut self) -> Option<&mut ImageLayer> {
        self.layers.iter_mut().find(|l| Some(l.id) == self.selected)
    }
    pub fn repair_selection(&mut self) {
        let exists = |id: Option<u64>| id.is_some_and(|id| self.layers.iter().any(|l| l.id == id));
        if !exists(self.selected) {
            self.selected = self.layers.last().map(|l| l.id);
        }
        if !exists(self.comparison.a) {
            self.comparison.a = self.layers.first().map(|l| l.id);
        }
        if !exists(self.comparison.b) {
            self.comparison.b = self.layers.last().map(|l| l.id);
        }
    }
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.version == 1,
            "Unsupported Pixel Perfecto document version"
        );
        anyhow::ensure!(self.layers.len() <= 256, "Too many layers");
        if let Some(m) = self.measurement {
            anyhow::ensure!(
                m.a.iter()
                    .chain(&m.b)
                    .all(|v| v.is_finite() && v.abs() <= 1e9)
                    && m.threshold.is_finite()
                    && (0.01..=0.8).contains(&m.threshold),
                "Invalid measurement"
            );
        }
        let mut guide_ids = std::collections::HashSet::new();
        anyhow::ensure!(self.rulers().guides.len() <= 1000, "Too many ruler guides");
        for guide in &self.rulers().guides {
            anyhow::ensure!(
                guide.id < u64::MAX
                    && guide_ids.insert(guide.id)
                    && guide.position.is_finite()
                    && guide.position.abs() <= 1e9,
                "Invalid ruler guide"
            );
        }
        anyhow::ensure!(
            self.viewport.center.iter().all(|v| v.is_finite())
                && (0.02..=4096.).contains(&self.viewport.zoom),
            "Invalid viewport"
        );
        anyhow::ensure!(
            (0.0..=1.0).contains(&self.grid.opacity)
                && (4.0..=128.0).contains(&self.grid.min_spacing),
            "Invalid grid settings"
        );
        anyhow::ensure!(
            (0.0..=1.0).contains(&self.comparison.wipe)
                && (0.1..=100.0).contains(&self.comparison.gain),
            "Invalid comparison settings"
        );
        let mut ids = std::collections::HashSet::new();
        for l in &self.layers {
            anyhow::ensure!(ids.insert(l.id), "Duplicate layer ID");
            anyhow::ensure!(
                l.size[0] > 0
                    && l.size[1] > 0
                    && l.size[0] as u64 * l.size[1] as u64 <= 100_000_000,
                "Invalid image dimensions"
            );
            anyhow::ensure!(
                (0.0..=1.0).contains(&l.opacity) && matches!(l.bit_depth, 8 | 16),
                "Invalid image settings"
            );
            l.calibration.validate(l.size)?;
            anyhow::ensure!(l.landmarks.len() <= 10_000, "Too many landmarks");
            anyhow::ensure!(
                l.landmarks
                    .iter()
                    .all(|p| p.source.iter().all(|v| v.is_finite())),
                "Invalid landmark"
            );
        }
        Ok(())
    }
}

#[derive(Default)]
pub struct History {
    undo: Vec<PixelPerfectoDocument>,
    redo: Vec<PixelPerfectoDocument>,
    last_recorded: Option<PixelPerfectoDocument>,
}
impl History {
    pub fn record(&mut self, before: PixelPerfectoDocument, after: &PixelPerfectoDocument) {
        if before == *after {
            return;
        }
        self.undo.push(before);
        self.redo.clear();
        self.last_recorded = Some(after.clone());
        if self.undo.len() > 100 {
            self.undo.remove(0);
        }
    }
    /// An asynchronous fit belongs to the anchor gesture if nothing was edited since it.
    pub fn record_followup(
        &mut self,
        before: PixelPerfectoDocument,
        after: &PixelPerfectoDocument,
    ) {
        if self.last_recorded.as_ref() == Some(&before) && !self.undo.is_empty() {
            self.last_recorded = Some(after.clone());
        } else {
            self.record(before, after);
        }
    }
    pub fn undo(&mut self, doc: &mut PixelPerfectoDocument) {
        self.last_recorded = None;
        if let Some(prev) = self.undo.pop() {
            self.redo.push(std::mem::replace(doc, prev));
        }
    }
    pub fn redo(&mut self, doc: &mut PixelPerfectoDocument) {
        self.last_recorded = None;
        if let Some(next) = self.redo.pop() {
            self.undo.push(std::mem::replace(doc, next));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn diamond_translation_preserves_every_site() {
        for i in -8..8 {
            for j in -8..8 {
                for kind in Subpixel::ALL {
                    let p = GridSite {
                        cell: [-3, 2],
                        kind,
                    }
                    .world();
                    let delta = cell_to_world([i as f64, j as f64]);
                    let site = GridSite::nearest(add(p, delta), kind);
                    assert_eq!(site.cell, [-3 + i, 2 + j]);
                    assert_eq!(site.kind, kind);
                }
            }
        }
        assert_eq!(
            GridSite {
                cell: [0, 0],
                kind: Subpixel::Blue
            }
            .world(),
            [0., 1.]
        );
        assert_eq!(
            GridSite {
                cell: [0, 0],
                kind: Subpixel::Green1
            }
            .world(),
            [0.5, 0.5]
        );
    }
    #[test]
    fn viewport_zoom_and_hidpi_do_not_change_snap() {
        for zoom in [0.25, 24., 96., 1024.] {
            let mut v = Viewport {
                center: [-2., 9.],
                zoom,
            };
            let size = [800., 600.];
            let p = [237., 411.];
            let before = v.screen_to_world(p, size);
            v.zoom_at(1.7, p, size);
            assert!(length(sub(before, v.screen_to_world(p, size))) < 1e-9);
            let delta = [5.3, -2.8];
            let screen = mul(delta, zoom * 2.);
            assert_eq!(
                snapped_delta(delta),
                snapped_delta(mul(screen, 1. / (zoom * 2.)))
            );
        }
    }
    #[test]
    fn undo_keeps_other_images_and_document_settings() {
        let mut d = PixelPerfectoDocument::default();
        let before = d.clone();
        let mut h = History::default();
        d.grid.opacity = 0.2;
        h.record(before.clone(), &d);
        h.undo(&mut d);
        assert_eq!(d, before);
        h.redo(&mut d);
        assert_eq!(d.grid.opacity, 0.2);
    }
}
