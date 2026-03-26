//! Pixel-driven curved CPR renderer.
//!
//! Unlike the strip-painting approach in `cpr.rs` (which iterates over
//! centerline points and paints outward), this module iterates over each
//! output pixel, finds the nearest projected centerline segment, and
//! samples the volume. This eliminates the overlap/gap fan artifacts
//! that plague strip-painting at curves.

use nalgebra::Vector3;
use ndarray::Array3;

use super::cpr::CurvedCprResult;
use super::interp::trilinear;

// ---------------------------------------------------------------------------
// 1. View basis from binormals
// ---------------------------------------------------------------------------

/// Compute an orthonormal view basis using PCA on the centerline positions.
///
/// The best-fit plane of the centerline is found via PCA. The viewing
/// direction is the normal to this plane (third principal component),
/// which minimizes self-intersection of the projected curve.
///
/// Returns `(view_forward, view_right, view_up)` where:
/// - `view_forward` points into the screen (normal to best-fit plane),
/// - `view_right` and `view_up` span the viewing plane.
///
/// Falls back to average-binormal if PCA is degenerate.
///
/// Coordinate convention: `[z, y, x]` — `Vector3::new(z, y, x)`.
pub fn compute_view_basis(
    binormals: &[Vector3<f64>],
) -> (Vector3<f64>, Vector3<f64>, Vector3<f64>) {
    assert!(!binormals.is_empty(), "need at least one binormal");

    // Fallback: average binormal
    let mut sum = Vector3::new(0.0, 0.0, 0.0);
    for b in binormals {
        sum += b;
    }
    let view_forward = (sum / binormals.len() as f64).normalize();

    let world_up = Vector3::new(1.0, 0.0, 0.0);

    let view_right = {
        let candidate = world_up.cross(&view_forward);
        if candidate.norm() > 1e-6 {
            candidate.normalize()
        } else {
            let fallback = Vector3::new(0.0, 1.0, 0.0);
            fallback.cross(&view_forward).normalize()
        }
    };

    let view_up = view_forward.cross(&view_right).normalize();

    (view_forward, view_right, view_up)
}

/// Compute view basis using PCA on centerline positions.
///
/// The third principal component (direction of least spread) becomes the
/// viewing direction, ensuring the projected 2D curve has minimal
/// self-intersection.
pub fn compute_view_basis_pca(
    positions: &[[f64; 3]],
) -> (Vector3<f64>, Vector3<f64>, Vector3<f64>) {
    let n = positions.len();
    assert!(n >= 2, "need at least 2 positions");

    // 1. Compute centroid
    let mut cx = 0.0;
    let mut cy = 0.0;
    let mut cz = 0.0;
    for p in positions {
        cx += p[0];
        cy += p[1];
        cz += p[2];
    }
    cx /= n as f64;
    cy /= n as f64;
    cz /= n as f64;

    // 2. Compute 3x3 covariance matrix
    let mut cov = nalgebra::Matrix3::<f64>::zeros();
    for p in positions {
        let d = Vector3::new(p[0] - cx, p[1] - cy, p[2] - cz);
        cov += d * d.transpose();
    }
    cov /= n as f64;

    // 3. Eigendecomposition — eigenvalues NOT sorted by nalgebra!
    let eig = cov.symmetric_eigen();

    // Find index of smallest eigenvalue (least variance = normal to curve's plane)
    let mut min_idx = 0;
    let mut min_val = eig.eigenvalues[0].abs();
    for i in 1..3 {
        if eig.eigenvalues[i].abs() < min_val {
            min_val = eig.eigenvalues[i].abs();
            min_idx = i;
        }
    }
    let view_forward = eig.eigenvectors.column(min_idx).normalize();

    // Use world_up to derive view_right
    let world_up = Vector3::new(1.0, 0.0, 0.0);

    let view_right = {
        let candidate = world_up.cross(&view_forward);
        if candidate.norm() > 1e-6 {
            candidate.normalize()
        } else {
            let fallback = Vector3::new(0.0, 1.0, 0.0);
            fallback.cross(&view_forward).normalize()
        }
    };

    let view_up = view_forward.cross(&view_right).normalize();

    (view_forward, view_right, view_up)
}

// ---------------------------------------------------------------------------
// 2. Project centerline to 2D
// ---------------------------------------------------------------------------

/// Project 3D centerline positions onto a 2D viewing plane.
///
/// Each position is projected as:
///   `x_mm = (pos − center) · view_right`
///   `y_mm = (pos − center) · view_up`
pub fn project_centerline_2d(
    positions: &[[f64; 3]],
    center: [f64; 3],
    view_right: &Vector3<f64>,
    view_up: &Vector3<f64>,
) -> Vec<(f64, f64)> {
    let c = Vector3::new(center[0], center[1], center[2]);
    positions
        .iter()
        .map(|p| {
            let v = Vector3::new(p[0], p[1], p[2]) - c;
            (v.dot(view_right), v.dot(view_up))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 3. Nearest point on projected centerline
// ---------------------------------------------------------------------------

/// Result of nearest-segment lookup on the 2D projected centerline.
#[allow(dead_code)]
pub struct NearestResult {
    /// Index of the segment start point (0-based).
    pub segment_idx: usize,
    /// Fraction [0, 1] along the segment from `segment_idx` to `segment_idx + 1`.
    pub segment_frac: f64,
    /// Signed perpendicular distance from the segment.
    /// Positive = to the *right* of the segment direction.
    pub signed_dist: f64,
}

/// Find the nearest segment on the projected 2D centerline to a query point.
///
/// For each consecutive pair `(P[j], P[j+1])`, the query is projected onto
/// the line segment and the closest is kept. The signed distance uses the 2D
/// perpendicular (segment direction rotated 90 degrees clockwise → right).
#[allow(dead_code)]
pub fn nearest_on_projected_centerline(
    projected: &[(f64, f64)],
    query_x: f64,
    query_y: f64,
) -> NearestResult {
    assert!(
        projected.len() >= 2,
        "need at least 2 projected points to form a segment"
    );

    let mut best_dist_sq = f64::MAX;
    let mut best_idx: usize = 0;
    let mut best_frac: f64 = 0.0;
    let mut best_signed: f64 = 0.0;

    for j in 0..projected.len() - 1 {
        let (ax, ay) = projected[j];
        let (bx, by) = projected[j + 1];

        let dx = bx - ax;
        let dy = by - ay;
        let seg_len_sq = dx * dx + dy * dy;

        // Fraction along segment (clamped to [0, 1])
        let frac = if seg_len_sq < 1e-30 {
            0.0
        } else {
            ((query_x - ax) * dx + (query_y - ay) * dy) / seg_len_sq
        }
        .clamp(0.0, 1.0);

        // Closest point on segment
        let cx = ax + frac * dx;
        let cy = ay + frac * dy;

        let diff_x = query_x - cx;
        let diff_y = query_y - cy;
        let dist_sq = diff_x * diff_x + diff_y * diff_y;

        if dist_sq < best_dist_sq {
            best_dist_sq = dist_sq;
            best_idx = j;
            best_frac = frac;

            // Signed distance: positive = right of segment direction.
            //
            // 2D cross: (query - A) × (B - A) = (qx-ax)*dy - (qy-ay)*dx
            //
            // In standard screen/image coords (y increasing downward), a
            // positive cross product means the query is to the LEFT of the
            // direction vector. In our projected mm coords (y increasing
            // upward, because view_up points up), a positive cross product
            // means the query is to the LEFT as well — but the "right"
            // perpendicular of (dx,dy) is (dy, -dx), and the dot product
            // of (query-closest) with that perpendicular equals cross/|seg|.
            //
            // So: signed_dist = cross / |segment| gives positive for the
            // side that (dy, -dx) points towards. For a rightward segment
            // (dx>0, dy=0) the right-perp is (0, -dx) i.e. -y, so a point
            // at -y (below) gets positive signed_dist → correct "right".
            let cross = (query_x - ax) * dy - (query_y - ay) * dx;
            let seg_len = seg_len_sq.sqrt();
            best_signed = if seg_len > 1e-15 {
                cross / seg_len
            } else {
                0.0
            };
        }
    }

    NearestResult {
        segment_idx: best_idx,
        segment_frac: best_frac,
        signed_dist: best_signed,
    }
}

// ---------------------------------------------------------------------------
// 4. Pixel-driven curved CPR renderer
// ---------------------------------------------------------------------------

/// Render a curved CPR using the pixel-driven approach.
///
/// For each output pixel the nearest projected centerline segment is found,
/// the corresponding 3D position and frame vectors are interpolated, and the
/// volume is sampled (with optional MIP slab along the binormal).
///
/// - `positions`, `normals`, `binormals`: already rotated by the caller.
/// - `arclengths`: cumulative arc-length at each centerline sample.
/// - `width_mm`: half-width of the lateral extent around the centerline.
/// - `slab_mm`: MIP slab thickness along the binormal (0 = single plane).
#[allow(dead_code)]
pub(crate) fn render_curved_cpr_pixeldriven(
    positions: &[[f64; 3]],
    normals: &[Vector3<f64>],
    binormals: &[Vector3<f64>],
    arclengths: &[f64],
    volume: &Array3<f32>,
    spacing: [f64; 3],
    origin: [f64; 3],
    width_mm: f64,
    pixels_wide: usize,
    pixels_high: usize,
    slab_mm: f64,
) -> CurvedCprResult {
    let n = positions.len();
    assert!(n >= 2, "need at least 2 centerline positions");
    assert_eq!(normals.len(), n);
    assert_eq!(binormals.len(), n);
    assert_eq!(arclengths.len(), n);
    assert!(pixels_wide >= 2 && pixels_high >= 2);

    // --- Step 1: view basis ---
    let (_view_forward, view_right, view_up) = compute_view_basis(binormals);

    // --- Step 2: project centerline to 2D ---
    let mid_idx = n / 2;
    let center = positions[mid_idx];
    let projected = project_centerline_2d(positions, center, &view_right, &view_up);

    // --- Step 3: bounding box + padding ---
    let mut min_x = f64::MAX;
    let mut max_x = f64::NEG_INFINITY;
    let mut min_y = f64::MAX;
    let mut max_y = f64::NEG_INFINITY;
    for &(px, py) in &projected {
        if px < min_x { min_x = px; }
        if px > max_x { max_x = px; }
        if py < min_y { min_y = py; }
        if py > max_y { max_y = py; }
    }
    min_x -= width_mm;
    max_x += width_mm;
    min_y -= width_mm;
    max_y += width_mm;

    let view_width = max_x - min_x;
    let view_height = max_y - min_y;

    // MIP slab offsets
    let n_slab_steps = if slab_mm > 0.01 { 9usize } else { 1 };
    let slab_offsets: Vec<f64> = if n_slab_steps > 1 {
        (0..n_slab_steps)
            .map(|k| {
                -slab_mm / 2.0
                    + slab_mm * (k as f64) / ((n_slab_steps - 1) as f64)
            })
            .collect()
    } else {
        vec![0.0]
    };

    let inv_spacing = [1.0 / spacing[0], 1.0 / spacing[1], 1.0 / spacing[2]];

    // --- Step 4: iterate over every output pixel ---
    //
    // KEY FIX: Convert each pixel to a 3D point on the viewing plane,
    // then compute lateral offset as dot product with the 3D normal.
    // This avoids the 2D-projection / 3D-normal mismatch that caused
    // banding artifacts in the previous version.
    let center_vec = Vector3::new(center[0], center[1], center[2]);
    let mut image = vec![f32::NAN; pixels_high * pixels_wide];

    for row in 0..pixels_high {
        for col in 0..pixels_wide {
            // (a) pixel → mm in the projected plane
            let x_mm = min_x + (col as f64) * view_width / ((pixels_wide - 1) as f64);
            let y_mm = max_y - (row as f64) * view_height / ((pixels_high - 1) as f64);

            // (b) nearest segment (use 2D projection for fast lookup)
            let nr = nearest_on_projected_centerline(&projected, x_mm, y_mm);

            // Quick reject: if 2D distance is far beyond width, skip
            if nr.signed_dist.abs() > width_mm * 1.5 {
                continue;
            }

            // (c) fractional centerline index
            let j = nr.segment_idx;
            let frac = nr.segment_frac;

            // (d) interpolate 3D position, normal, binormal
            let j1 = (j + 1).min(n - 1);
            let interp_pos = Vector3::new(
                positions[j][0] + frac * (positions[j1][0] - positions[j][0]),
                positions[j][1] + frac * (positions[j1][1] - positions[j][1]),
                positions[j][2] + frac * (positions[j1][2] - positions[j][2]),
            );
            let interp_normal = lerp_vec3(&normals[j], &normals[j1], frac).normalize();
            let interp_binormal = lerp_vec3(&binormals[j], &binormals[j1], frac).normalize();

            // (e) Convert pixel to 3D point on the viewing plane
            let pixel_3d = center_vec + x_mm * view_right + y_mm * view_up;

            // (f) 3D offset from centerline to pixel position
            let offset_3d = pixel_3d - interp_pos;

            // (g) Lateral offset = component along the 3D normal
            let lateral_offset = offset_3d.dot(&interp_normal);

            // (h) Clip to lateral width
            if lateral_offset.abs() > width_mm {
                continue;
            }

            // (h2) Reject mis-assigned pixels from projection fold-back.
            // The depth component (along binormal / view_forward) of a
            // correctly-assigned pixel on the viewing plane should be near
            // zero. If it's large, the 2D lookup matched a distant segment
            // that only appears close in projection.
            let depth_offset = offset_3d.dot(&interp_binormal).abs();
            if depth_offset > width_mm {
                continue;
            }

            // (i) 3D sample: centerline + lateral along normal
            let sample_base = interp_pos + lateral_offset * interp_normal;

            // (j) MIP across slab along binormal
            let mut max_val = f32::NEG_INFINITY;
            for &slab_off in &slab_offsets {
                let sample_mm = sample_base + slab_off * interp_binormal;

                let vz = (sample_mm[0] - origin[0]) * inv_spacing[0];
                let vy = (sample_mm[1] - origin[1]) * inv_spacing[1];
                let vx = (sample_mm[2] - origin[2]) * inv_spacing[2];

                let val = trilinear(volume, vz, vy, vx);
                if !val.is_nan() && val > max_val {
                    max_val = val;
                }
            }

            image[row * pixels_wide + col] = if max_val == f32::NEG_INFINITY {
                f32::NAN
            } else {
                max_val
            };
        }
    }

    CurvedCprResult {
        image,
        pixels_wide,
        pixels_high,
        arclengths: arclengths.to_vec(),
    }
}

// ---------------------------------------------------------------------------
// 5. Warp-from-straightened curved CPR renderer
// ---------------------------------------------------------------------------

/// Render a curved CPR by warping a straightened CPR using 3D nearest-point lookup.
///
/// For each output pixel on the viewing plane, the nearest centerline point is
/// found in 3D (not 2D). The arc-length fraction gives the straightened column,
/// the perpendicular distance gives the row. This is immune to projection
/// fold-back because the 3D distance is unambiguous.
pub(crate) fn render_curved_from_straightened(
    straight_image: &[f32],
    straight_w: usize,
    straight_h: usize,
    positions: &[[f64; 3]],
    normals: &[Vector3<f64>],
    width_mm: f64,
    pixels_wide: usize,
    pixels_high: usize,
    arclengths: &[f64],
) -> CurvedCprResult {
    let n = positions.len();
    assert!(n >= 2);
    assert_eq!(normals.len(), n);

    // 1. View basis via PCA for the output viewing plane
    let (_vf, view_right, view_up) = compute_view_basis_pca(positions);

    // 2. Project centerline to 2D (only for bounding box + output layout)
    let mid_idx = n / 2;
    let center = positions[mid_idx];
    let center_vec = Vector3::new(center[0], center[1], center[2]);
    let projected = project_centerline_2d(positions, center, &view_right, &view_up);

    // 3. Bounding box + padding
    let mut min_x = f64::MAX;
    let mut max_x = f64::NEG_INFINITY;
    let mut min_y = f64::MAX;
    let mut max_y = f64::NEG_INFINITY;
    for &(px, py) in &projected {
        if px < min_x { min_x = px; }
        if px > max_x { max_x = px; }
        if py < min_y { min_y = py; }
        if py > max_y { max_y = py; }
    }
    min_x -= width_mm;
    max_x += width_mm;
    min_y -= width_mm;
    max_y += width_mm;

    let view_width = max_x - min_x;
    let view_height = max_y - min_y;

    // 4. Precompute position vectors for 3D distance search
    let pos_vecs: Vec<Vector3<f64>> = positions.iter()
        .map(|p| Vector3::new(p[0], p[1], p[2]))
        .collect();

    // 5. For each output pixel: find nearest centerline point in 3D,
    // then look up the straightened CPR value.
    let mut image = vec![f32::NAN; pixels_high * pixels_wide];

    for row in 0..pixels_high {
        for col in 0..pixels_wide {
            // Pixel → 3D position on viewing plane
            let x_mm = min_x + (col as f64) * view_width / ((pixels_wide - 1) as f64);
            let y_mm = max_y - (row as f64) * view_height / ((pixels_high - 1) as f64);
            let pixel_3d = center_vec + x_mm * view_right + y_mm * view_up;

            // Find nearest centerline point in 3D (brute force)
            let mut best_j = 0usize;
            let mut best_dist_sq = f64::MAX;
            for j in 0..n {
                let d = (pixel_3d - pos_vecs[j]).norm_squared();
                if d < best_dist_sq {
                    best_dist_sq = d;
                    best_j = j;
                }
            }

            // Compute lateral offset along the normal at this point
            let offset = pixel_3d - pos_vecs[best_j];
            let lateral = offset.dot(&normals[best_j]);

            if lateral.abs() > width_mm {
                continue;
            }

            // Map to straightened CPR coordinates
            let src_col = (best_j as f64 / (n - 1) as f64) * (straight_w - 1) as f64;
            let src_row = (0.5 - lateral / (2.0 * width_mm)) * (straight_h - 1) as f64;

            if src_col < 0.0 || src_col > (straight_w - 1) as f64
                || src_row < 0.0 || src_row > (straight_h - 1) as f64
            {
                continue;
            }

            // Bilinear lookup in straightened image
            let c0 = src_col.floor() as usize;
            let r0 = src_row.floor() as usize;
            let c1 = (c0 + 1).min(straight_w - 1);
            let r1 = (r0 + 1).min(straight_h - 1);
            let wc = (src_col - c0 as f64) as f32;
            let wr = (src_row - r0 as f64) as f32;

            let v00 = straight_image[r0 * straight_w + c0];
            let v01 = straight_image[r0 * straight_w + c1];
            let v10 = straight_image[r1 * straight_w + c0];
            let v11 = straight_image[r1 * straight_w + c1];

            if v00.is_nan() || v01.is_nan() || v10.is_nan() || v11.is_nan() {
                let vals = [v00, v01, v10, v11];
                let valid: Vec<f32> = vals.iter().copied().filter(|v| !v.is_nan()).collect();
                if !valid.is_empty() {
                    image[row * pixels_wide + col] = valid.iter().sum::<f32>() / valid.len() as f32;
                }
            } else {
                let top = v00 * (1.0 - wc) + v01 * wc;
                let bot = v10 * (1.0 - wc) + v11 * wc;
                image[row * pixels_wide + col] = top * (1.0 - wr) + bot * wr;
            }
        }
    }

    CurvedCprResult {
        image,
        pixels_wide,
        pixels_high,
        arclengths: arclengths.to_vec(),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Linear interpolation between two `Vector3<f64>` values.
#[inline]
#[allow(dead_code)]
fn lerp_vec3(a: &Vector3<f64>, b: &Vector3<f64>, t: f64) -> Vector3<f64> {
    a * (1.0 - t) + b * t
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- A. test_view_basis_orthogonal --

    #[test]
    fn test_view_basis_orthogonal() {
        // A collection of varied binormals
        let binormals = vec![
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(0.1, 0.0, 1.0).normalize(),
            Vector3::new(-0.1, 0.0, 1.0).normalize(),
        ];

        let (vf, vr, vu) = compute_view_basis(&binormals);

        // All three should be unit vectors
        assert!(
            (vf.norm() - 1.0).abs() < 1e-6,
            "view_forward not unit: {}",
            vf.norm()
        );
        assert!(
            (vr.norm() - 1.0).abs() < 1e-6,
            "view_right not unit: {}",
            vr.norm()
        );
        assert!(
            (vu.norm() - 1.0).abs() < 1e-6,
            "view_up not unit: {}",
            vu.norm()
        );

        // Pairwise orthogonality
        assert!(
            vf.dot(&vr).abs() < 1e-6,
            "forward·right = {}",
            vf.dot(&vr)
        );
        assert!(
            vf.dot(&vu).abs() < 1e-6,
            "forward·up = {}",
            vf.dot(&vu)
        );
        assert!(
            vr.dot(&vu).abs() < 1e-6,
            "right·up = {}",
            vr.dot(&vu)
        );
    }

    // -- B. test_projection_straight_line_z --

    #[test]
    fn test_projection_straight_line_z() {
        // Straight line along z-axis at (y=32, x=32)
        let positions: Vec<[f64; 3]> = (0..50)
            .map(|z| [z as f64, 32.0, 32.0])
            .collect();
        let center = positions[25];

        // For a line along z, tangent = [1,0,0], bishop normal ≈ [0,1,0],
        // binormal ≈ [0,0,1]. Average binormal → view_forward = [0,0,1].
        // view_right = world_up × view_forward = [1,0,0] × [0,0,1] = [0,-1,0]
        // (or its normalised form). view_up = view_forward × view_right.
        // The exact vectors depend on the cross product signs, but we can
        // check that all projected x values are approximately zero (the line
        // is straight so it should project to a single line in 2D).
        let binormals: Vec<Vector3<f64>> = (0..50)
            .map(|_| Vector3::new(0.0, 0.0, 1.0))
            .collect();

        let (_vf, vr, vu) = compute_view_basis(&binormals);
        let proj = project_centerline_2d(&positions, center, &vr, &vu);

        // All projected x values should be near zero (line is straight)
        for (i, &(px, _py)) in proj.iter().enumerate() {
            assert!(
                px.abs() < 1e-6,
                "point {} projected x should be ~0, got {}",
                i,
                px
            );
        }

        // y values should be increasing (or decreasing — just monotonic)
        let mut increasing = true;
        let mut decreasing = true;
        for i in 1..proj.len() {
            if proj[i].1 <= proj[i - 1].1 + 1e-10 {
                increasing = false;
            }
            if proj[i].1 >= proj[i - 1].1 - 1e-10 {
                decreasing = false;
            }
        }
        assert!(
            increasing || decreasing,
            "projected y values should be monotonic"
        );
    }

    // -- C. test_nearest_on_centerline_exact --

    #[test]
    fn test_nearest_on_centerline_exact() {
        // Straight horizontal centerline in 2D
        let projected: Vec<(f64, f64)> = (0..10)
            .map(|i| (i as f64 * 5.0, 0.0))
            .collect();

        // Query a point exactly on the centerline
        let nr = nearest_on_projected_centerline(&projected, 12.5, 0.0);

        assert!(
            nr.signed_dist.abs() < 1e-6,
            "signed_dist should be ~0 for on-centerline point, got {}",
            nr.signed_dist
        );
    }

    // -- D. test_nearest_off_centerline --

    #[test]
    fn test_nearest_off_centerline() {
        // Straight horizontal centerline at y=0
        let projected: Vec<(f64, f64)> = (0..10)
            .map(|i| (i as f64 * 5.0, 0.0))
            .collect();

        // Query 5mm above the centerline (positive y direction)
        // For a segment going in +x direction, the right-perpendicular
        // points in -y. So a point at +y is to the LEFT → signed_dist < 0.
        // A point at -y is to the RIGHT → signed_dist > 0.
        let nr = nearest_on_projected_centerline(&projected, 20.0, -5.0);

        assert!(
            (nr.signed_dist - 5.0).abs() < 1e-6,
            "signed_dist should be ~5.0, got {}",
            nr.signed_dist
        );
    }

    // -- E. test_nearest_segment_frac --

    #[test]
    fn test_nearest_segment_frac() {
        // Two-point segment: (0,0) → (10,0)
        let projected = vec![(0.0, 0.0), (10.0, 0.0)];

        // Query midpoint
        let nr = nearest_on_projected_centerline(&projected, 5.0, 0.0);

        assert_eq!(nr.segment_idx, 0);
        assert!(
            (nr.segment_frac - 0.5).abs() < 1e-6,
            "frac should be ~0.5, got {}",
            nr.segment_frac
        );
        assert!(
            nr.signed_dist.abs() < 1e-6,
            "signed_dist should be ~0, got {}",
            nr.signed_dist
        );
    }

    // -- F. test_straight_line_pixeldriven_matches_values --

    #[test]
    fn test_straight_line_pixeldriven_matches_values() {
        // Volume: value = z coordinate
        let mut vol = Array3::<f32>::zeros((64, 64, 64));
        for z in 0..64 {
            for y in 0..64 {
                for x in 0..64 {
                    vol[[z, y, x]] = z as f32;
                }
            }
        }
        let spacing = [1.0, 1.0, 1.0];
        let origin = [0.0, 0.0, 0.0];

        // Straight centerline along z-axis at (y=32, x=32)
        let n_pts = 50;
        let positions: Vec<[f64; 3]> = (0..n_pts)
            .map(|z| [5.0 + z as f64, 32.0, 32.0]) // start at z=5, end at z=54
            .collect();

        // Bishop frame for a z-axis line: T=[1,0,0], N=[0,1,0], B=[0,0,1]
        let normals: Vec<Vector3<f64>> = (0..n_pts)
            .map(|_| Vector3::new(0.0, 1.0, 0.0))
            .collect();
        let binormals: Vec<Vector3<f64>> = (0..n_pts)
            .map(|_| Vector3::new(0.0, 0.0, 1.0))
            .collect();
        let arclengths: Vec<f64> = (0..n_pts).map(|i| i as f64).collect();

        let result = render_curved_cpr_pixeldriven(
            &positions,
            &normals,
            &binormals,
            &arclengths,
            &vol,
            spacing,
            origin,
            10.0, // width_mm
            64,   // pixels_wide
            64,   // pixels_high
            0.0,  // no slab
        );

        assert_eq!(result.image.len(), 64 * 64);

        // For a straight Z-line the centerline projects vertically in the
        // output image (because view_up aligns with Z). Therefore a center
        // *column* traverses along the centerline and should show increasing
        // Z (= increasing value in our gradient volume).
        let mid_col = result.pixels_wide / 2;
        let col_vals: Vec<f32> = (0..result.pixels_high)
            .map(|r| result.image[r * result.pixels_wide + mid_col])
            .collect();

        // Collect non-NaN values
        let valid: Vec<f32> = col_vals.iter().copied().filter(|v| !v.is_nan()).collect();
        assert!(
            valid.len() > 5,
            "expected many valid pixels in center column, got {}",
            valid.len()
        );

        // The column goes from top (row 0 = max_y = high z) to bottom
        // (last row = min_y = low z), so values should be DECREASING
        // (or we just check monotonic in one direction with tolerance).
        // Actually they could be increasing or decreasing depending on
        // view_up sign. Just check monotonicity in either direction.
        let mut inc_violations = 0;
        let mut dec_violations = 0;
        for i in 1..valid.len() {
            if valid[i] < valid[i - 1] - 0.5 {
                inc_violations += 1;
            }
            if valid[i] > valid[i - 1] + 0.5 {
                dec_violations += 1;
            }
        }
        assert!(
            inc_violations == 0 || dec_violations == 0,
            "center column values should be monotonic: inc_violations={}, dec_violations={}",
            inc_violations,
            dec_violations
        );

        // Values should span a good range (from ~5 to ~54)
        let vmin = valid.iter().cloned().fold(f32::INFINITY, f32::min);
        let vmax = valid.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        assert!(
            vmax - vmin > 20.0,
            "expected wide range of z-values, got {} to {}",
            vmin,
            vmax
        );
    }

    // -- G. test_quarter_circle_no_nan_gaps --

    #[test]
    fn test_quarter_circle_no_nan_gaps() {
        // Volume: uniform value = 100.0 everywhere
        let vol = Array3::<f32>::from_elem((64, 64, 64), 100.0);
        let spacing = [1.0, 1.0, 1.0];
        let origin = [0.0, 0.0, 0.0];

        // Quarter-circle centerline in the z-y plane, centered at (32,32,32)
        let n_pts = 60;
        let radius = 20.0;
        let mut positions = Vec::with_capacity(n_pts);
        let mut normals = Vec::with_capacity(n_pts);
        let mut binormals = Vec::with_capacity(n_pts);
        let mut arclengths = Vec::with_capacity(n_pts);

        for i in 0..n_pts {
            let theta = std::f64::consts::FRAC_PI_2 * (i as f64) / ((n_pts - 1) as f64);
            let z = 32.0 + radius * theta.cos();
            let y = 32.0 + radius * theta.sin();
            let x = 32.0;
            positions.push([z, y, x]);

            // Tangent = d/dtheta [cos, sin, 0] = [-sin, cos, 0]
            let tz = -theta.sin();
            let ty = theta.cos();
            let tangent = Vector3::new(tz, ty, 0.0).normalize();

            // Normal = inward radial = -[cos, sin, 0] (pointing toward center)
            let normal = Vector3::new(-theta.cos(), -theta.sin(), 0.0).normalize();

            // Binormal = tangent × normal
            let binormal = tangent.cross(&normal).normalize();

            normals.push(normal);
            binormals.push(binormal);

            let arc = radius * theta;
            arclengths.push(arc);
        }

        let width_mm = 8.0;
        let px_w = 80;
        let px_h = 80;

        let result = render_curved_cpr_pixeldriven(
            &positions,
            &normals,
            &binormals,
            &arclengths,
            &vol,
            spacing,
            origin,
            width_mm,
            px_w,
            px_h,
            0.0,
        );

        assert_eq!(result.image.len(), px_w * px_h);

        // Count pixels that are within the lateral width of the centerline
        // and should therefore NOT be NaN. We re-project and check.
        let (_vf, vr, vu) = compute_view_basis(&binormals);
        let center = positions[n_pts / 2];
        let projected = project_centerline_2d(&positions, center, &vr, &vu);

        // Compute bounding box (same logic as renderer)
        let mut min_x = f64::MAX;
        let mut max_x = f64::NEG_INFINITY;
        let mut min_y = f64::MAX;
        let mut max_y = f64::NEG_INFINITY;
        for &(px, py) in &projected {
            if px < min_x { min_x = px; }
            if px > max_x { max_x = px; }
            if py < min_y { min_y = py; }
            if py > max_y { max_y = py; }
        }
        min_x -= width_mm;
        max_x += width_mm;
        min_y -= width_mm;
        max_y += width_mm;
        let vw = max_x - min_x;
        let vh = max_y - min_y;

        let mut nan_inside_count = 0;
        let mut inside_count = 0;

        for row in 0..px_h {
            for col in 0..px_w {
                let x_mm = min_x + (col as f64) * vw / ((px_w - 1) as f64);
                let y_mm = max_y - (row as f64) * vh / ((px_h - 1) as f64);

                let nr = nearest_on_projected_centerline(&projected, x_mm, y_mm);

                // Only check pixels well inside the width (use 90% margin
                // to avoid edge effects)
                if nr.signed_dist.abs() < width_mm * 0.9 {
                    inside_count += 1;
                    let val = result.image[row * px_w + col];
                    if val.is_nan() {
                        nan_inside_count += 1;
                    }
                }
            }
        }

        assert!(
            inside_count > 100,
            "expected many pixels inside the vessel width, got {}",
            inside_count
        );
        assert_eq!(
            nan_inside_count, 0,
            "pixel-driven curved CPR should have NO NaN gaps inside the vessel width, \
             but found {}/{} NaN pixels",
            nan_inside_count, inside_count
        );
    }
}
