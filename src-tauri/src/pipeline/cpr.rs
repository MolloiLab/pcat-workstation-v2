use nalgebra::Vector3;
use ndarray::Array3;

use super::interp::trilinear;

/// Result of a CPR computation.
#[derive(serde::Serialize)]
pub struct CprResult {
    /// Flattened row-major CPR image, shape (pixels_high, pixels_wide)
    pub image: Vec<f32>,
    pub pixels_wide: usize,  // arc-length axis (columns)
    pub pixels_high: usize,  // lateral axis (rows)
    pub arclengths: Vec<f64>, // pixels_wide entries, mm
}

/// Result of a cross-section computation.
#[derive(serde::Serialize)]
pub struct CrossSectionResult {
    pub image: Vec<f32>,  // pixels x pixels, row-major
    pub pixels: usize,
    pub arc_mm: f64,      // arc-length position
}

// ---------------------------------------------------------------------------
// Internal helpers: centerline resampling & Bishop frame
// ---------------------------------------------------------------------------

/// Uniformly resample a dense centerline (already at ~0.5mm spacing) to
/// exactly `n` positions along arc length. Returns (positions, tangents, arclengths).
///
/// Tangents are computed using centered finite differences on the RESAMPLED
/// positions for smoothness, not from the original polyline segments.
pub fn resample_centerline(
    pts: &[[f64; 3]],
    n: usize,
) -> (Vec<Vector3<f64>>, Vec<Vector3<f64>>, Vec<f64>) {
    assert!(pts.len() >= 2, "centerline must have at least 2 points");
    assert!(n >= 2, "need at least 2 output samples");

    // Compute cumulative arc lengths
    let mut cum_arc = Vec::with_capacity(pts.len());
    cum_arc.push(0.0);
    for i in 1..pts.len() {
        let d = Vector3::new(
            pts[i][0] - pts[i - 1][0],
            pts[i][1] - pts[i - 1][1],
            pts[i][2] - pts[i - 1][2],
        )
        .norm();
        cum_arc.push(cum_arc[i - 1] + d);
    }
    let total_arc = *cum_arc.last().unwrap();

    let mut positions = Vec::with_capacity(n);
    let mut arclengths = Vec::with_capacity(n);

    // Pointer into the source polyline for O(n+m) walk
    let mut seg = 0usize;

    for j in 0..n {
        let s = total_arc * (j as f64) / ((n - 1) as f64);
        arclengths.push(s);

        // Advance segment pointer
        while seg + 1 < pts.len() - 1 && cum_arc[seg + 1] < s {
            seg += 1;
        }

        // Interpolate position within segment [seg, seg+1]
        let seg_len = cum_arc[seg + 1] - cum_arc[seg];
        let t = if seg_len > 1e-12 {
            ((s - cum_arc[seg]) / seg_len).clamp(0.0, 1.0)
        } else {
            0.0
        };

        let p0 = Vector3::new(pts[seg][0], pts[seg][1], pts[seg][2]);
        let p1 = Vector3::new(pts[seg + 1][0], pts[seg + 1][1], pts[seg + 1][2]);
        positions.push(p0 + t * (p1 - p0));
    }

    // Compute smooth tangents using centered finite differences on resampled positions
    let mut tangents = Vec::with_capacity(n);
    for j in 0..n {
        let tang = if j == 0 {
            (positions[1] - positions[0]).normalize()
        } else if j == n - 1 {
            (positions[n - 1] - positions[n - 2]).normalize()
        } else {
            (positions[j + 1] - positions[j - 1]).normalize()
        };
        tangents.push(tang);
    }

    (positions, tangents, arclengths)
}

/// Compute Bishop (parallel-transport) frame along a sequence of tangent
/// vectors. Returns (normals, binormals).
pub fn bishop_frame(tangents: &[Vector3<f64>]) -> (Vec<Vector3<f64>>, Vec<Vector3<f64>>) {
    let n = tangents.len();
    let mut normals = Vec::with_capacity(n);
    let mut binormals = Vec::with_capacity(n);

    // Choose initial normal perpendicular to T[0]
    let t0 = tangents[0];
    let world_y = Vector3::new(0.0, 1.0, 0.0);
    let world_x = Vector3::new(1.0, 0.0, 0.0);

    let seed = if t0.cross(&world_y).norm() > 0.1 {
        world_y
    } else {
        world_x
    };
    let n0 = t0.cross(&seed).normalize();
    let b0 = t0.cross(&n0).normalize();
    normals.push(n0);
    binormals.push(b0);

    // Parallel transport: project previous normal onto the plane perp to current tangent
    for i in 1..n {
        let t = tangents[i];
        let prev_n = normals[i - 1];

        // Remove component along T[i]
        let projected = prev_n - prev_n.dot(&t) * t;
        let pn = projected.norm();
        let ni = if pn > 1e-12 {
            projected / pn
        } else {
            // Degenerate: tangent changed drastically, re-seed
            let s = if t.cross(&world_y).norm() > 0.1 {
                world_y
            } else {
                world_x
            };
            t.cross(&s).normalize()
        };
        let bi = t.cross(&ni).normalize();
        normals.push(ni);
        binormals.push(bi);
    }

    (normals, binormals)
}

/// Apply rotation around the tangent axis to normals and binormals.
fn rotate_frame(
    normals: &mut [Vector3<f64>],
    binormals: &mut [Vector3<f64>],
    rotation_deg: f64,
) {
    if rotation_deg.abs() < 1e-10 {
        return;
    }
    let theta = rotation_deg.to_radians();
    let cos_t = theta.cos();
    let sin_t = theta.sin();

    for i in 0..normals.len() {
        let n = normals[i];
        let b = binormals[i];
        normals[i] = cos_t * n + sin_t * b;
        binormals[i] = -sin_t * n + cos_t * b;
    }
}

// ---------------------------------------------------------------------------
// Fixed up-vector frame (stable alternative to Bishop for CPR)
// ---------------------------------------------------------------------------

/// Compute a frame using a fixed world up-vector projected onto the plane
/// perpendicular to each tangent. This avoids the error accumulation of
/// parallel-transport (Bishop frame) and produces more stable CPR images.
/// The tradeoff is slight twist along straight sections, but this is how
/// Horos computes CPR and it works well in practice.
fn fixed_up_frame(tangents: &[Vector3<f64>]) -> (Vec<Vector3<f64>>, Vec<Vector3<f64>>) {
    let up = Vector3::new(0.0, 0.0, 1.0); // world Z up
    let alt_up = Vector3::new(0.0, 1.0, 0.0); // fallback if tangent ~ parallel to Z

    tangents
        .iter()
        .map(|t| {
            // Project up onto plane perpendicular to tangent
            let projected = up - up.dot(t) * t;
            let n = if projected.norm() > 0.1 {
                projected.normalize()
            } else {
                // Tangent nearly parallel to Z, use world Y instead
                let alt_proj = alt_up - alt_up.dot(t) * t;
                alt_proj.normalize()
            };
            let b = t.cross(&n).normalize();
            (n, b)
        })
        .unzip()
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Compute a Curved Planar Reformation (CPR) image from a volume along a
/// centerline.
///
/// - `volume`: 3D array of HU values, shape (Z, Y, X).
/// - `centerline_mm`: Dense centerline points in [z, y, x] mm.
/// - `spacing`: Volume spacing [sz, sy, sx] mm.
/// - `origin`: Volume origin [oz, oy, ox] mm.
/// - `width_mm`: Half-width of the lateral axis in mm.
/// - `slab_mm`: MIP slab thickness in mm.
/// - `pixels_wide`: Output width (arc-length axis / columns).
/// - `pixels_high`: Output height (lateral axis / rows).
/// - `rotation_deg`: Rotational CPR angle in degrees.
pub fn compute_cpr(
    volume: &Array3<f32>,
    centerline_mm: &[[f64; 3]],
    spacing: [f64; 3],
    origin: [f64; 3],
    width_mm: f64,
    slab_mm: f64,
    pixels_wide: usize,
    pixels_high: usize,
    rotation_deg: f64,
) -> CprResult {
    // 1. Resample centerline to pixels_wide uniform arc-length positions
    let (positions, tangents, arclengths) = resample_centerline(centerline_mm, pixels_wide);

    // 2. Fixed up-vector frame (stable, no error accumulation)
    let (mut normals, mut binormals) = fixed_up_frame(&tangents);

    // 3. Rotation
    rotate_frame(&mut normals, &mut binormals, rotation_deg);

    // 4. MIP slab sampling parameters — 9 steps for smoother edges
    let n_slab_steps = if slab_mm > 0.01 { 9usize } else { 1 };
    let slab_offsets: Vec<f64> = if n_slab_steps > 1 {
        (0..n_slab_steps)
            .map(|k| -slab_mm / 2.0 + slab_mm * (k as f64) / ((n_slab_steps - 1) as f64))
            .collect()
    } else {
        vec![0.0]
    };

    // Inverse spacing for mm -> voxel conversion
    let inv_spacing = [1.0 / spacing[0], 1.0 / spacing[1], 1.0 / spacing[2]];

    // 5. Image reconstruction
    let mut image = vec![f32::NAN; pixels_high * pixels_wide];

    for j in 0..pixels_wide {
        let pos = positions[j];
        let n_vec = normals[j];
        let b_vec = binormals[j];

        for i in 0..pixels_high {
            // Lateral offset: top row = +width_mm, bottom row = -width_mm
            let lateral = width_mm * (1.0 - 2.0 * (i as f64) / ((pixels_high - 1) as f64));

            let mut max_val = f32::NEG_INFINITY;

            for &slab_off in &slab_offsets {
                let sample_mm = pos + lateral * n_vec + slab_off * b_vec;

                // Convert mm -> voxel space: voxel = (mm - origin) / spacing
                let vz = (sample_mm[0] - origin[0]) * inv_spacing[0];
                let vy = (sample_mm[1] - origin[1]) * inv_spacing[1];
                let vx = (sample_mm[2] - origin[2]) * inv_spacing[2];

                let val = trilinear(volume, vz, vy, vx);
                if !val.is_nan() && val > max_val {
                    max_val = val;
                }
            }

            image[i * pixels_wide + j] = if max_val == f32::NEG_INFINITY {
                f32::NAN
            } else {
                max_val
            };
        }
    }

    CprResult {
        image,
        pixels_wide,
        pixels_high,
        arclengths,
    }
}

/// Compute a cross-sectional image perpendicular to the centerline at a
/// given arc-length position.
///
/// - `volume`: 3D array of HU values, shape (Z, Y, X).
/// - `centerline_mm`: Dense centerline points in [z, y, x] mm.
/// - `spacing`: Volume spacing [sz, sy, sx] mm.
/// - `origin`: Volume origin [oz, oy, ox] mm.
/// - `position_frac`: Fractional position along the centerline [0.0, 1.0].
/// - `rotation_deg`: Rotational CPR angle in degrees.
/// - `width_mm`: Half-width of the cross-section in mm.
/// - `pixels`: Output square image size (pixels x pixels).
pub fn compute_cross_section(
    volume: &Array3<f32>,
    centerline_mm: &[[f64; 3]],
    spacing: [f64; 3],
    origin: [f64; 3],
    position_frac: f64,
    rotation_deg: f64,
    width_mm: f64,
    pixels: usize,
) -> CrossSectionResult {
    // We need positions + frame at just one point, but reuse the same
    // resampling to get consistent arc-length parametrisation.
    let n_samples = centerline_mm.len().max(2);
    let (positions, tangents, arclengths) = resample_centerline(centerline_mm, n_samples);
    let (mut normals, mut binormals) = bishop_frame(&tangents);
    rotate_frame(&mut normals, &mut binormals, rotation_deg);

    // Map fractional position to index
    let idx = ((position_frac * (n_samples - 1) as f64).round() as usize).min(n_samples - 1);

    let pos = positions[idx];
    let n_vec = normals[idx];
    let b_vec = binormals[idx];
    let arc_mm = arclengths[idx];

    let inv_spacing = [1.0 / spacing[0], 1.0 / spacing[1], 1.0 / spacing[2]];

    let mut image = vec![f32::NAN; pixels * pixels];

    for row in 0..pixels {
        for col in 0..pixels {
            // Map (row, col) to physical offsets along N and B
            let offset_n = width_mm * (1.0 - 2.0 * (row as f64) / ((pixels - 1) as f64));
            let offset_b = width_mm * (1.0 - 2.0 * (col as f64) / ((pixels - 1) as f64));

            let sample_mm = pos + offset_n * n_vec + offset_b * b_vec;

            // Convert mm -> voxel space: voxel = (mm - origin) / spacing
            let vz = (sample_mm[0] - origin[0]) * inv_spacing[0];
            let vy = (sample_mm[1] - origin[1]) * inv_spacing[1];
            let vx = (sample_mm[2] - origin[2]) * inv_spacing[2];

            image[row * pixels + col] = trilinear(volume, vz, vy, vx);
        }
    }

    CrossSectionResult {
        image,
        pixels,
        arc_mm,
    }
}

/// Compute multiple cross-sections in a single call, sharing the centerline
/// resampling and Bishop frame computation. Much faster than calling
/// `compute_cross_section` separately for each position.
pub fn compute_cross_sections_batch(
    volume: &Array3<f32>,
    centerline_mm: &[[f64; 3]],
    spacing: [f64; 3],
    origin: [f64; 3],
    position_fracs: &[f64],
    rotation_deg: f64,
    width_mm: f64,
    pixels: usize,
) -> Vec<CrossSectionResult> {
    // Resample + Bishop frame ONCE
    let n_samples = centerline_mm.len().max(2);
    let (positions, tangents, arclengths) = resample_centerline(centerline_mm, n_samples);
    let (mut normals, mut binormals) = bishop_frame(&tangents);
    rotate_frame(&mut normals, &mut binormals, rotation_deg);

    let inv_spacing = [1.0 / spacing[0], 1.0 / spacing[1], 1.0 / spacing[2]];

    // Compute each cross-section using the shared precomputed frame
    position_fracs
        .iter()
        .map(|&frac| {
            let idx =
                ((frac * (n_samples - 1) as f64).round() as usize).min(n_samples - 1);

            let pos = positions[idx];
            let n_vec = normals[idx];
            let b_vec = binormals[idx];
            let arc_mm = arclengths[idx];

            let mut image = vec![f32::NAN; pixels * pixels];

            for row in 0..pixels {
                for col in 0..pixels {
                    let offset_n =
                        width_mm * (1.0 - 2.0 * (row as f64) / ((pixels - 1) as f64));
                    let offset_b =
                        width_mm * (1.0 - 2.0 * (col as f64) / ((pixels - 1) as f64));

                    let sample_mm = pos + offset_n * n_vec + offset_b * b_vec;

                    let vz = (sample_mm[0] - origin[0]) * inv_spacing[0];
                    let vy = (sample_mm[1] - origin[1]) * inv_spacing[1];
                    let vx = (sample_mm[2] - origin[2]) * inv_spacing[2];

                    image[row * pixels + col] = trilinear(volume, vz, vy, vx);
                }
            }

            CrossSectionResult {
                image,
                pixels,
                arc_mm,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ndarray::Array3;

    fn make_test_volume() -> (Array3<f32>, [f64; 3]) {
        // 64^3 volume, value = z coordinate (so CPR along z axis shows gradient)
        let mut vol = Array3::<f32>::zeros((64, 64, 64));
        for z in 0..64 {
            for y in 0..64 {
                for x in 0..64 {
                    vol[[z, y, x]] = z as f32;
                }
            }
        }
        let spacing = [1.0, 1.0, 1.0];
        (vol, spacing)
    }

    #[test]
    fn test_compute_cpr_basic() {
        let (vol, spacing) = make_test_volume();

        // Straight centerline along z-axis at (y=32, x=32)
        let centerline: Vec<[f64; 3]> = (0..60)
            .map(|z| [z as f64, 32.0, 32.0])
            .collect();

        let origin = [0.0, 0.0, 0.0];
        let result = compute_cpr(
            &vol,
            &centerline,
            spacing,
            origin,
            10.0,  // width_mm
            0.0,   // no slab
            60,    // pixels_wide
            21,    // pixels_high
            0.0,   // no rotation
        );

        assert_eq!(result.pixels_wide, 60);
        assert_eq!(result.pixels_high, 21);
        assert_eq!(result.image.len(), 60 * 21);
        assert_eq!(result.arclengths.len(), 60);

        // Center row (row 10) should roughly follow z values
        let center_row: Vec<f32> = (0..60)
            .map(|j| result.image[10 * 60 + j])
            .collect();
        // Values should be increasing (following z gradient)
        for j in 1..58 {
            if !center_row[j].is_nan() && !center_row[j - 1].is_nan() {
                assert!(
                    center_row[j] >= center_row[j - 1] - 1.0,
                    "center row should increase: j={}, val={}, prev={}",
                    j, center_row[j], center_row[j - 1]
                );
            }
        }
    }

    #[test]
    fn test_compute_cross_section_basic() {
        let (vol, spacing) = make_test_volume();

        let centerline: Vec<[f64; 3]> = (0..60)
            .map(|z| [z as f64, 32.0, 32.0])
            .collect();

        let origin = [0.0, 0.0, 0.0];
        let result = compute_cross_section(
            &vol,
            &centerline,
            spacing,
            origin,
            0.5,   // middle of the centerline
            0.0,   // no rotation
            10.0,  // width_mm
            21,    // pixels
        );

        assert_eq!(result.pixels, 21);
        assert_eq!(result.image.len(), 21 * 21);
        // At mid-centerline (z ~30), all cross-section pixels should be ~30
        let center_val = result.image[10 * 21 + 10];
        if !center_val.is_nan() {
            assert!(
                (center_val - 30.0).abs() < 2.0,
                "center value should be ~30, got {}",
                center_val
            );
        }
    }

    #[test]
    fn test_resample_preserves_endpoints() {
        let pts: Vec<[f64; 3]> = vec![
            [0.0, 0.0, 0.0],
            [10.0, 0.0, 0.0],
            [20.0, 0.0, 0.0],
        ];
        let (positions, _tangents, arclengths) = resample_centerline(&pts, 5);

        assert_eq!(positions.len(), 5);
        assert!((positions[0] - Vector3::new(0.0, 0.0, 0.0)).norm() < 1e-10);
        assert!((positions[4] - Vector3::new(20.0, 0.0, 0.0)).norm() < 1e-10);
        assert!((arclengths[0]).abs() < 1e-10);
        assert!((arclengths[4] - 20.0).abs() < 1e-10);
    }
}
