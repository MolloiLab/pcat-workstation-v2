//! Vessel wall / lumen geometry on a 2-D cross-section image.
//!
//! Per-ray radial half-max detection (FWHM) anchored on the image centre.
//! The lumen peak is taken as the max HU along the inner half of each ray,
//! so the threshold is invariant to whatever bright structures (aorta,
//! chambers, calcium) happen to sit inside the 30 mm FOV — they can't pull
//! the threshold up the way a global histogram would.

/// Per-angle vessel radii plus the derived equivalent-circle diameter and
/// boundary polygon.
pub struct VesselGeometry {
    /// Equivalent-circle diameter in mm, `D = 2·√(A/π)` where A is the polar
    /// integral of the per-angle radii.
    pub diameter_mm: f64,
    /// Boundary polygon in image pixel coords `[x, y]`, one point per angle,
    /// starting at θ = 0 (east) and walking counter-clockwise in image space
    /// (y-axis inverted, i.e. top-left origin).
    pub wall: Vec<[f64; 2]>,
    /// Raw radius at each angle in mm (same ordering as `wall`).
    pub r_theta: Vec<f64>,
}

/// Compute the lumen boundary + diameter from a cross-section image.
///
/// * `image` — row-major f32 HU, `pixels × pixels`.
/// * `pixels` — image side length.
/// * `width_mm` — physical half-width of the image (image spans
///   `2·width_mm` on each side; matches the `width_mm` used by
///   `CprFrame::render_cross_section`).
/// * `n_angles` — number of radial rays (polygon vertex count).
pub fn compute_vessel_geometry(
    image: &[f32],
    pixels: usize,
    width_mm: f64,
    n_angles: usize,
) -> VesselGeometry {
    let mm_per_pixel = 2.0 * width_mm / pixels as f64;
    let center_px = pixels as f64 / 2.0;

    let r_theta = detect_vessel_wall_2d(image, pixels, mm_per_pixel, n_angles);

    let wall: Vec<[f64; 2]> = (0..n_angles)
        .map(|ai| {
            let theta = 2.0 * std::f64::consts::PI * (ai as f64) / (n_angles as f64);
            let r_px = r_theta[ai] / mm_per_pixel;
            let x = center_px + r_px * theta.cos();
            let y = center_px - r_px * theta.sin(); // top-left origin
            [x, y]
        })
        .collect();

    // Polar-integrated area: A = 0.5 · Σ r² · dθ.
    let dtheta = 2.0 * std::f64::consts::PI / (n_angles as f64);
    let area_mm2: f64 = 0.5 * r_theta.iter().map(|&r| r * r).sum::<f64>() * dtheta;
    let diameter_mm = 2.0 * (area_mm2 / std::f64::consts::PI).sqrt();

    VesselGeometry {
        diameter_mm,
        wall,
        r_theta,
    }
}

/// Detect vessel wall on a 2-D cross-section image using per-ray half-max
/// threshold. Returns `r_theta[i]` — boundary radius in mm at each of
/// `n_angles` directions.
pub fn detect_vessel_wall_2d(
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

        let r_px = detect_boundary_2d(&profile, radial_step_px, max_radius_px);
        r_theta.push(r_px * mm_per_pixel);
    }

    r_theta
}

/// Detect lumen boundary from a radial HU profile on the cross-section image.
///
/// Strategy: half-max threshold with steepest-gradient refinement.
///
/// * Lumen HU peak: max of the inner half of the profile (near centre, so
///   bright structures elsewhere in the FOV cannot contaminate it).
/// * Background: fixed at `-100 HU` (soft-tissue / fat surround — the
///   actual environment of a coronary artery).
/// * Threshold: mid-point of `(peak, background)`.
fn detect_boundary_2d(profile: &[f64], step_px: f64, max_radius_px: f64) -> f64 {
    if profile.is_empty() {
        return max_radius_px;
    }

    let search_len = (profile.len() / 2).max(3).min(profile.len());
    let max_hu = profile[..search_len]
        .iter()
        .cloned()
        .fold(f64::NEG_INFINITY, f64::max);

    if max_hu < 50.0 {
        // No lumen signal; contrast is too weak to locate a wall.
        return max_radius_px;
    }

    let background = -100.0;
    let threshold = (max_hu + background) / 2.0;

    // First crossing outward.
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

    // Refine to steepest negative gradient near the crossing.
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

    v00 * (1.0 - fx) * (1.0 - fy)
        + v10 * fx * (1.0 - fy)
        + v01 * (1.0 - fx) * fy
        + v11 * fx * fy
}
