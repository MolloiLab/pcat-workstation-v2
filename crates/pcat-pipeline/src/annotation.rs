use ndarray::Array3;
use serde::Serialize;

use crate::active_contour::init_circular_contour;
use crate::cpr::CprFrame;

/// A single cross-section prepared for annotation.
#[derive(Debug, Clone, Serialize)]
pub struct AnnotationTarget {
    /// Cross-section image (HU), row-major, pixels x pixels.
    pub image: Vec<f32>,
    /// Image dimension in pixels.
    pub pixels: usize,
    /// Physical width of the cross-section in mm (from center to edge on each side).
    pub width_mm: f64,
    /// Arc-length position along the centerline in mm.
    pub arc_mm: f64,
    /// Frame column index this cross-section corresponds to.
    pub frame_index: usize,
    /// Auto-detected vessel wall contour as [x,y] pixel coordinates.
    pub vessel_wall: Vec<[f64; 2]>,
    /// Vessel equivalent radius in mm (sqrt(area/pi)).
    pub vessel_radius_mm: f64,
    /// Initial snake boundary: circle at ~2x vessel equivalent radius from center, in [x,y] pixel coords.
    pub init_boundary: Vec<[f64; 2]>,
}

/// Parameters for generating annotation targets.
#[derive(Debug, Clone)]
pub struct AnnotationBatchParams {
    /// Number of cross-sections (default: 20).
    pub n_sections: usize,
    /// Spacing between cross-sections in mm (default: 2.0).
    pub section_spacing_mm: f64,
    /// Cross-section image half-width in mm (default: 15.0).
    pub width_mm: f64,
    /// Cross-section image pixel size (default: 128).
    pub pixels: usize,
    /// Number of points on snake contour (default: 72).
    pub n_snake_points: usize,
}

impl Default for AnnotationBatchParams {
    fn default() -> Self {
        Self {
            n_sections: 20,
            section_spacing_mm: 2.0,
            width_mm: 15.0,
            pixels: 128,
            n_snake_points: 72,
        }
    }
}

/// Generate annotation targets for a segment of the centerline.
///
/// Takes a precomputed CprFrame and produces `n_sections` cross-section images
/// at equal spacing, with auto-detected vessel walls and initial snake boundaries.
pub fn generate_annotation_batch(
    frame: &CprFrame,
    volume: &Array3<f32>,
    spacing: [f64; 3],
    origin: [f64; 3],
    direction: &[f64; 9],
    params: &AnnotationBatchParams,
) -> Vec<AnnotationTarget> {
    let n_cols = frame.n_cols();
    if n_cols < 2 {
        return Vec::new();
    }

    let total_arc = frame.arclengths[n_cols - 1];
    let n_angles = 360usize;
    let mm_per_pixel = 2.0 * params.width_mm / params.pixels as f64;
    let center_px = params.pixels as f64 / 2.0;

    let mut targets = Vec::with_capacity(params.n_sections);

    for section in 0..params.n_sections {
        let arc_mm = section as f64 * params.section_spacing_mm;

        // Stop if we exceed the centerline length.
        if arc_mm > total_arc {
            break;
        }

        // Find nearest frame column index for this arc-length.
        let frame_index = find_nearest_arc_index(&frame.arclengths, arc_mm);

        // Render cross-section at this position.
        let position_frac = frame_index as f64 / (n_cols - 1) as f64;
        let cs = frame.render_cross_section(
            volume,
            spacing,
            origin,
            direction,
            position_frac,
            0.0, // rotation_deg = 0
            params.width_mm,
            params.pixels,
        );

        // Auto-detect vessel wall on the 2D cross-section image.
        let r_theta = detect_vessel_wall_2d(&cs.image, params.pixels, mm_per_pixel, n_angles);

        // Convert polar contour to Cartesian pixel coordinates.
        let vessel_wall: Vec<[f64; 2]> = (0..n_angles)
            .map(|ai| {
                let theta = 2.0 * std::f64::consts::PI * (ai as f64) / (n_angles as f64);
                let r_px = r_theta[ai] / mm_per_pixel;
                let x = center_px + r_px * theta.cos();
                let y = center_px - r_px * theta.sin(); // y-axis inverted (top-left origin)
                [x, y]
            })
            .collect();

        // Compute vessel equivalent radius: r_eq = sqrt(area / pi)
        // Area via polar integration: A = 0.5 * sum(r^2) * dtheta
        let dtheta = 2.0 * std::f64::consts::PI / (n_angles as f64);
        let area_mm2: f64 = 0.5 * r_theta.iter().map(|&r| r * r).sum::<f64>() * dtheta;
        let vessel_radius_mm = (area_mm2 / std::f64::consts::PI).sqrt();

        // Initial snake boundary: circle at 2 * r_eq from center (1 diameter out from wall).
        let init_radius_mm = (2.0 * vessel_radius_mm).min(params.width_mm * 0.9);
        let init_radius_px = init_radius_mm / mm_per_pixel;
        let init_boundary =
            init_circular_contour(center_px, center_px, init_radius_px, params.n_snake_points);

        targets.push(AnnotationTarget {
            image: cs.image,
            pixels: params.pixels,
            width_mm: params.width_mm,
            arc_mm: cs.arc_mm,
            frame_index,
            vessel_wall,
            vessel_radius_mm,
            init_boundary,
        });
    }

    targets
}

/// Find the index in `arclengths` closest to `target_arc`.
fn find_nearest_arc_index(arclengths: &[f64], target_arc: f64) -> usize {
    let mut best_idx = 0;
    let mut best_diff = f64::INFINITY;
    for (i, &s) in arclengths.iter().enumerate() {
        let diff = (s - target_arc).abs();
        if diff < best_diff {
            best_diff = diff;
            best_idx = i;
        }
    }
    best_idx
}

/// Detect vessel wall on a 2D cross-section image using radial half-max threshold.
///
/// Returns `r_theta`: boundary radius in mm at each of `n_angles` directions.
fn detect_vessel_wall_2d(
    image: &[f32],
    pixels: usize,
    mm_per_pixel: f64,
    n_angles: usize,
) -> Vec<f64> {
    let center = pixels as f64 / 2.0;
    let max_radius_px = center; // sample to edge of image
    let radial_step_px = 0.5; // half-pixel steps for precision
    let n_radial = (max_radius_px / radial_step_px) as usize;

    let mut r_theta = Vec::with_capacity(n_angles);

    for ai in 0..n_angles {
        let theta = 2.0 * std::f64::consts::PI * (ai as f64) / (n_angles as f64);
        let cos_t = theta.cos();
        let sin_t = theta.sin();

        // Sample radial HU profile from center outward.
        let mut profile = Vec::with_capacity(n_radial);
        for ri in 0..n_radial {
            let r_px = (ri as f64) * radial_step_px;
            let x = center + r_px * cos_t;
            let y = center - r_px * sin_t; // y inverted

            let hu = sample_image_bilinear(image, pixels, x, y);
            profile.push(if hu.is_nan() { -1024.0 } else { hu as f64 });
        }

        // Detect boundary using half-max threshold.
        let r_px = detect_boundary_2d(&profile, radial_step_px, max_radius_px);
        r_theta.push(r_px * mm_per_pixel);
    }

    r_theta
}

/// Detect lumen boundary from a radial HU profile on the cross-section image.
///
/// Strategy: half-max threshold with steepest gradient refinement (same as contour.rs).
fn detect_boundary_2d(profile: &[f64], step_px: f64, max_radius_px: f64) -> f64 {
    if profile.is_empty() {
        return max_radius_px;
    }

    // Find max HU in first half of profile (lumen near center).
    let search_len = (profile.len() / 2).max(3).min(profile.len());
    let max_hu = profile[..search_len]
        .iter()
        .cloned()
        .fold(f64::NEG_INFINITY, f64::max);

    if max_hu < 50.0 {
        // No lumen signal.
        return max_radius_px;
    }

    // Half-max threshold (background ~ -100 HU for soft tissue).
    let background = -100.0;
    let threshold = (max_hu + background) / 2.0;

    // Walk outward from center to find half-max crossing.
    let mut crossing_idx = None;
    for i in 1..profile.len() {
        if profile[i] < threshold {
            crossing_idx = Some(i);
            break;
        }
    }

    let crossing = match crossing_idx {
        Some(idx) => idx,
        None => return max_radius_px,
    };

    // Steepest negative gradient in a window around the crossing.
    let window_start = crossing.saturating_sub(3);
    let window_end = (crossing + 3).min(profile.len() - 1);

    let mut steepest_idx = crossing;
    let mut steepest_grad = 0.0_f64;

    for i in window_start..window_end {
        let grad = profile[i + 1] - profile[i];
        if grad < steepest_grad {
            steepest_grad = grad;
            steepest_idx = i;
        }
    }

    let r = (steepest_idx as f64 + 0.5) * step_px;
    r.clamp(0.5, max_radius_px)
}

/// Bilinear interpolation on a row-major f32 image.
fn sample_image_bilinear(image: &[f32], pixels: usize, x: f64, y: f64) -> f32 {
    let max_coord = (pixels as f64) - 1.0;
    if x < 0.0 || y < 0.0 || x > max_coord || y > max_coord {
        return f32::NAN;
    }

    let x = x.clamp(0.0, max_coord);
    let y = y.clamp(0.0, max_coord);

    let x0 = x.floor() as usize;
    let y0 = y.floor() as usize;
    let x1 = (x0 + 1).min(pixels - 1);
    let y1 = (y0 + 1).min(pixels - 1);

    let fx = (x - x0 as f64) as f32;
    let fy = (y - y0 as f64) as f32;

    let v00 = image[y0 * pixels + x0];
    let v10 = image[y0 * pixels + x1];
    let v01 = image[y1 * pixels + x0];
    let v11 = image[y1 * pixels + x1];

    v00 * (1.0 - fx) * (1.0 - fy) + v10 * fx * (1.0 - fy) + v01 * (1.0 - fx) * fy + v11 * fx * fy
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpr::CprFrame;
    use ndarray::Array3;

    /// Create a synthetic volume with a contrast-enhanced tube (cylinder) along z-axis
    /// centered at (y=32, x=32). The tube has HU=300 inside, -100 outside.
    fn make_vessel_phantom() -> (Array3<f32>, [f64; 3], [f64; 3]) {
        let size = 64;
        let mut vol = Array3::<f32>::from_elem((size, size, size), -100.0);
        let center_y = 32.0_f64;
        let center_x = 32.0_f64;
        let vessel_radius = 3.0_f64; // 3mm radius vessel

        for z in 0..size {
            for y in 0..size {
                for x in 0..size {
                    let dy = y as f64 - center_y;
                    let dx = x as f64 - center_x;
                    let dist = (dy * dy + dx * dx).sqrt();
                    if dist <= vessel_radius {
                        vol[[z, y, x]] = 300.0;
                    }
                }
            }
        }

        let spacing = [1.0, 1.0, 1.0];
        let origin = [0.0, 0.0, 0.0];
        (vol, spacing, origin)
    }

    /// Build a CprFrame along the center of the phantom vessel.
    fn make_frame(n_cols: usize) -> CprFrame {
        let centerline_mm: Vec<[f64; 3]> = (2..62)
            .map(|z| [z as f64, 32.0, 32.0])
            .collect();
        CprFrame::from_centerline(&centerline_mm, n_cols)
    }

    #[test]
    fn test_correct_count() {
        let (vol, spacing, origin) = make_vessel_phantom();
        let frame = make_frame(200);
        let params = AnnotationBatchParams::default();
        let targets = generate_annotation_batch(&frame, &vol, spacing, origin, &crate::types::IDENTITY_DIRECTION, &params);
        assert_eq!(
            targets.len(),
            params.n_sections,
            "Should produce exactly {} targets, got {}",
            params.n_sections,
            targets.len()
        );
    }

    #[test]
    fn test_arc_length_spacing() {
        let (vol, spacing, origin) = make_vessel_phantom();
        let frame = make_frame(200);
        let params = AnnotationBatchParams::default();
        let targets = generate_annotation_batch(&frame, &vol, spacing, origin, &crate::types::IDENTITY_DIRECTION, &params);

        // Verify consecutive targets are spaced at approximately section_spacing_mm.
        for i in 1..targets.len() {
            let delta = targets[i].arc_mm - targets[i - 1].arc_mm;
            assert!(
                (delta - params.section_spacing_mm).abs() < 1.0,
                "Spacing between target {} and {}: {:.2} mm, expected ~{:.1} mm",
                i - 1,
                i,
                delta,
                params.section_spacing_mm
            );
        }
    }

    #[test]
    fn test_vessel_wall_exists() {
        let (vol, spacing, origin) = make_vessel_phantom();
        let frame = make_frame(200);
        let params = AnnotationBatchParams::default();
        let targets = generate_annotation_batch(&frame, &vol, spacing, origin, &crate::types::IDENTITY_DIRECTION, &params);

        for (i, t) in targets.iter().enumerate() {
            assert!(
                !t.vessel_wall.is_empty(),
                "Target {} should have a non-empty vessel wall contour",
                i
            );
            assert!(
                t.vessel_radius_mm > 0.0,
                "Target {} should have positive vessel radius, got {}",
                i,
                t.vessel_radius_mm
            );
        }
    }

    #[test]
    fn test_init_boundary_larger_than_vessel() {
        let (vol, spacing, origin) = make_vessel_phantom();
        let frame = make_frame(200);
        let params = AnnotationBatchParams::default();
        let targets = generate_annotation_batch(&frame, &vol, spacing, origin, &crate::types::IDENTITY_DIRECTION, &params);

        let mm_per_pixel = 2.0 * params.width_mm / params.pixels as f64;
        let center_px = params.pixels as f64 / 2.0;

        for (i, t) in targets.iter().enumerate() {
            // Average radius of init_boundary in pixels from center.
            let avg_init_r_px: f64 = t
                .init_boundary
                .iter()
                .map(|p| {
                    let dx = p[0] - center_px;
                    let dy = p[1] - center_px;
                    (dx * dx + dy * dy).sqrt()
                })
                .sum::<f64>()
                / t.init_boundary.len() as f64;

            let vessel_r_px = t.vessel_radius_mm / mm_per_pixel;

            assert!(
                avg_init_r_px > vessel_r_px,
                "Target {}: init_boundary avg radius ({:.1} px) should be > vessel radius ({:.1} px)",
                i,
                avg_init_r_px,
                vessel_r_px
            );
        }
    }

    #[test]
    fn test_image_dimensions_correct() {
        let (vol, spacing, origin) = make_vessel_phantom();
        let frame = make_frame(200);
        let params = AnnotationBatchParams::default();
        let targets = generate_annotation_batch(&frame, &vol, spacing, origin, &crate::types::IDENTITY_DIRECTION, &params);

        for (i, t) in targets.iter().enumerate() {
            assert_eq!(
                t.image.len(),
                t.pixels * t.pixels,
                "Target {}: image length {} != pixels^2 = {}",
                i,
                t.image.len(),
                t.pixels * t.pixels
            );
            assert_eq!(t.pixels, params.pixels);
        }
    }
}
