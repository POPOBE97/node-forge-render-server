//! Explicit offline per-photograph calibration. Never touches hardware or session geometry.
use super::{controller, geometry, measurement, model::*, raw::RawFrame, repository::Repository};
use anyhow::{Context, Result, ensure};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize)]
pub struct RecalibrationResult {
    pub id: String,
    pub source_capture: String,
    pub capture: Option<String>,
    pub reference: Option<String>,
    pub geometry_holdout_px: Option<f64>,
    pub valid_patches: usize,
    pub total_patches: usize,
    pub transform: Option<ColorTransform>,
    pub transform_issue: Option<String>,
    pub issue: Option<String>,
    pub output: PathBuf,
}

fn check_source(source: &StoredCapture) -> Result<()> {
    ensure!(
        source.ticket.workflow == Workflow::Reference,
        "Select a reference RAW"
    );
    ensure!(
        source.display_issue.is_none(),
        "Cannot recalibrate a capture with an invalid display state: {}",
        source.display_issue.as_deref().unwrap_or_default()
    );
    Ok(())
}

/// Append a new analysis/reference version, retaining the original capture and failed run.
/// A transform fitted from valid patches can be exported as a diagnostic even when a complete
/// 60-patch baseline cannot be published. Its status and incomplete patch count remain explicit.
pub fn reference(root: &Path, source_id: &str) -> Result<RecalibrationResult> {
    let mut repo = Repository::open(root)?;
    let source = repo.capture(source_id)?;
    check_source(&source)?;
    let path = repo.verify_raw(&source)?;
    let id = new_id("recalibration");
    let output = repo.root.join("results").join(&id);
    std::fs::create_dir_all(&output)?;
    let mut result = RecalibrationResult {
        id,
        source_capture: source_id.into(),
        capture: None,
        reference: None,
        geometry_holdout_px: None,
        valid_patches: 0,
        total_patches: source.ticket.conditions.manifest.patches.len(),
        transform: None,
        transform_issue: None,
        issue: None,
        output,
    };
    let processing = (|| -> Result<()> {
        let raw = RawFrame::decode(&path)?;
        let detected = geometry::detect(&raw, &result.output)?;
        let profile = geometry::fit(
            &raw,
            &source.ticket.conditions.manifest,
            &detected,
            &source.id,
        )?;
        result.geometry_holdout_px = Some(profile.holdout_rms);
        let mut ticket = source.ticket.clone();
        ticket.id = new_id("individual-recalibration");
        ticket.geometry = Some(profile.clone());
        let capture = controller::analyze(
            &repo,
            ticket,
            path,
            source.camera_record.clone(),
            source.display_issue.clone(),
        )?;
        result.capture = Some(capture.id.clone());
        if let Some(issue) = &capture.analysis_issue {
            anyhow::bail!("{issue}");
        }
        capture.ensure_geometry_verified()?;
        let measured = capture
            .measurements
            .as_ref()
            .context("Missing RAW measurements")?;
        result.valid_patches = measured.patches.iter().filter(|p| p.valid).count();
        let manifest = &capture.ticket.conditions.manifest;
        match measurement::color_transform(measured, manifest) {
            Ok(transform) => {
                let folder = result.output.join("color");
                std::fs::create_dir_all(&folder)?;
                measurement::export(&raw, &profile, manifest, Some(transform.matrix), &folder)?;
                result.transform = Some(transform);
            }
            Err(error) => result.transform_issue = Some(format!("{error:#}")),
        }
        // Publication uses the same full-patch and geometric evidence checks as the UI.
        let baseline = measurement::reference(
            format!(
                "{} · {} · brightness {:.1} · individual geometry",
                capture.ticket.conditions.batch,
                manifest.mode.label(),
                capture.ticket.conditions.brightness
            ),
            &[capture],
        )?;
        repo.insert_reference(&baseline)?;
        result.reference = Some(baseline.id);
        Ok(())
    })();
    if let Err(error) = processing {
        result.issue = Some(format!("{error:#}"));
    }
    repo.write_json(
        &PathBuf::from("results")
            .join(&result.id)
            .join("result.json"),
        &result,
    )?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_recalibration_cannot_clear_a_display_failure() {
        let mut capture = crate::screen_test::tests::capture();
        capture.display_issue = Some("Phone brightness changed during exposure".into());
        assert!(check_source(&capture).is_err());
        capture.display_issue = None;
        capture.analysis_issue = Some("Shared geometry no longer matches".into());
        assert!(check_source(&capture).is_ok());
        capture.ticket.workflow = Workflow::Inspection;
        assert!(check_source(&capture).is_err());
    }
}
