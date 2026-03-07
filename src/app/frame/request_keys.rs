use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};

use crate::app::{ClippingSettings, DiffMetricMode, RefImageMode, types::AnalysisSourceDomain};

fn hash_key<T: Hash + ?Sized>(value: &T) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AnalysisSourceKey(u64);

impl AnalysisSourceKey {
    pub fn from_source(source: &AnalysisSourceDomain<'_>) -> Self {
        Self(hash_key(&(source.texture_name, source.size, source.format)))
    }

    #[cfg(test)]
    pub(crate) fn from_hashable<T: Hash + ?Sized>(value: &T) -> Self {
        Self(hash_key(value))
    }

    pub fn with_diff_request(self, diff_request_key: Option<DiffRequestKey>) -> Self {
        Self(hash_key(&(
            self.0,
            diff_request_key.map(Self::raw_from_diff),
        )))
    }

    pub fn raw(self) -> u64 {
        self.0
    }

    fn raw_from_diff(diff_request_key: DiffRequestKey) -> u64 {
        diff_request_key.raw()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DiffRequestKey(u64);

impl DiffRequestKey {
    pub fn new(
        source_key: AnalysisSourceKey,
        reference_size: [u32; 2],
        reference_offset: [i32; 2],
        reference_mode: RefImageMode,
        reference_opacity_bits: u32,
        metric_mode: DiffMetricMode,
        clamp_output: bool,
    ) -> Self {
        Self(hash_key(&(
            source_key.raw(),
            reference_size,
            reference_offset,
            reference_mode,
            reference_opacity_bits,
            metric_mode,
            clamp_output,
        )))
    }

    pub fn raw(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DiffStatsRequestKey(u64);

impl DiffStatsRequestKey {
    pub fn new(diff_key: DiffRequestKey) -> Self {
        Self(hash_key(&(diff_key.raw(), "stats")))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HistogramRequestKey(u64);

impl HistogramRequestKey {
    pub fn new(source_key: AnalysisSourceKey) -> Self {
        Self(hash_key(&(source_key.raw(), "histogram")))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ParadeRequestKey(u64);

impl ParadeRequestKey {
    pub fn new(source_key: AnalysisSourceKey) -> Self {
        Self(hash_key(&(source_key.raw(), "parade")))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct VectorscopeRequestKey(u64);

impl VectorscopeRequestKey {
    pub fn new(source_key: AnalysisSourceKey) -> Self {
        Self(hash_key(&(source_key.raw(), "vectorscope")))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ClippingRequestKey(u64);

impl ClippingRequestKey {
    pub fn new(source_key: AnalysisSourceKey, settings: ClippingSettings, enabled: bool) -> Self {
        Self(hash_key(&(
            source_key.raw(),
            enabled,
            settings.shadow_threshold.to_bits(),
            settings.highlight_threshold.to_bits(),
        )))
    }
}
