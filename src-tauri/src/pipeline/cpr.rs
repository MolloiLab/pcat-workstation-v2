use nalgebra::Vector3;
use ndarray::Array3;

use super::interp::trilinear;
use super::spline::CubicSpline3D;

/// Result of a CPR computation.
#[derive(serde::Serialize)]
pub struct CprResult {
    /// Flattened row-major CPR image, shape (pixels_high, pixels_wide)
    pub image: Vec<f32>,
    pub pixels_wide: usize,  // arc-length axis (columns)
    pub pixels_high: usize,  // lateral axis (rows)
    pub arclengths: Vec<f64>, // pixels_wide entries, mm
}

/// Result of a curved CPR computation.
pub struct CurvedCprResult {
    /// Flattened row-major image, shape (pixels_high, pixels_wide).
    /// Pixels outside the vessel field of view are NAN.
    pub image: Vec<f32>,
    pub pixels_wide: usize,
    pub pixels_high: usize,
    pub arclengths: Vec<f64>,
}

/// Result of a cross-section computation.
#[derive(serde::Serialize)]
pub struct CrossSectionResult {
    pub image: Vec<f32>,  // pixels x pixels, row-major
    pub pixels: usize,
    pub arc_mm: f64,      // arc-length position
}

// ---------------------------------------------------------------------------
// CprFrame — precomputed per-centerline, cached for reuse
// ---------------------------------------------------------------------------

/// Precomputed CPR frame data — cached in AppState for reuse across
/// render_cpr and render_cross_section calls at different rotation angles.
///
/// Built once when the centerline changes (~100ms), then reused for
/// rotation/needle changes (<10ms per render).
#[allow(dead_code)]
pub struct CprFrame {
    pub positions: Vec<[f64; 3]>,    // [z,y,x] in mm, n_cols entries
    pub tangents: Vec<Vector3<f64>>,
    pub normals: Vec<Vector3<f64>>,  // RMF (Double Reflection Method)
    pub binormals: Vec<Vector3<f64>>,
    pub arclengths: Vec<f64>,
}

impl CprFrame {
    /// Build frame from centerline points. Call once per centerline change.
    ///
    /// - `points`: centerline in [z,y,x] mm (dense, from frontend)
    /// - `n_cols`: number of uniformly-spaced samples along arc-length
    pub fn from_centerline(points: &[[f64; 3]], n_cols: usize) -> Self {
        assert!(points.len() >= 2, "centerline must have at least 2 points");
        assert!(n_cols >= 2, "need at least 2 output columns");

        // 1. Fit cubic spline through the centerline points
        let spline = CubicSpline3D::fit(points);
        let total_arc = spline.total_arc();

        // 2. Sample at n_cols uniform arc-length positions
        let mut positions = Vec::with_capacity(n_cols);
        let mut arclengths = Vec::with_capacity(n_cols);
        let mut tangents = Vec::with_capacity(n_cols);

        for j in 0..n_cols {
            let s = total_arc * (j as f64) / ((n_cols - 1) as f64);
            arclengths.push(s);

            let pos = spline.eval(s);
            positions.push(pos);

            // Analytic tangent — smooth, no noise from finite differences
            let t = spline.tangent(s);
            tangents.push(Vector3::new(t[0], t[1], t[2]));
        }

        // 3. Compute Bishop frame (parallel transport) from smooth tangents
        let (normals, binormals) = bishop_frame(&tangents);

        Self {
            positions,
            tangents,
            normals,
            binormals,
            arclengths,
        }
    }

    /// Number of columns (arc-length samples) in this frame.
    pub fn n_cols(&self) -> usize {
        self.positions.len()
    }

    /// Compute CPR image at a given rotation angle. Fast — just rotates + samples.
    ///
    /// - `slab_mm`: 0.0 for single-plane (interactive), >0 for MIP slab
    pub fn render_cpr(
        &self,
        volume: &Array3<f32>,
        spacing: [f64; 3],
        origin: [f64; 3],
        rotation_deg: f64,
        width_mm: f64,
        pixels_high: usize,
        slab_mm: f64,
    ) -> CprResult {
        let n_cols = self.n_cols();

        // Rotate the Bishop frame by the given angle
        let (rot_normals, rot_binormals) = self.rotated_frame(rotation_deg);

        // MIP slab sampling parameters
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

        // Image reconstruction
        let mut image = vec![f32::NAN; pixels_high * n_cols];

        for j in 0..n_cols {
            let pos = Vector3::new(
                self.positions[j][0],
                self.positions[j][1],
                self.positions[j][2],
            );
            let n_vec = rot_normals[j];
            let b_vec = rot_binormals[j];

            for i in 0..pixels_high {
                // Lateral offset: top row = +width_mm, bottom row = -width_mm
                let lateral =
                    width_mm * (1.0 - 2.0 * (i as f64) / ((pixels_high - 1) as f64));

                let mut max_val = f32::NEG_INFINITY;

                for &slab_off in &slab_offsets {
                    let sample_mm = pos + lateral * n_vec + slab_off * b_vec;

                    let vz = (sample_mm[0] - origin[0]) * inv_spacing[0];
                    let vy = (sample_mm[1] - origin[1]) * inv_spacing[1];
                    let vx = (sample_mm[2] - origin[2]) * inv_spacing[2];

                    let val = trilinear(volume, vz, vy, vx);
                    if !val.is_nan() && val > max_val {
                        max_val = val;
                    }
                }

                image[i * n_cols + j] = if max_val == f32::NEG_INFINITY {
                    f32::NAN
                } else {
                    max_val
                };
            }
        }

        CprResult {
            image,
            pixels_wide: n_cols,
            pixels_high,
            arclengths: self.arclengths.clone(),
        }
    }

    /// Compute cross-section at a fractional position along the centerline.
    pub fn render_cross_section(
        &self,
        volume: &Array3<f32>,
        spacing: [f64; 3],
        origin: [f64; 3],
        position_frac: f64,
        rotation_deg: f64,
        width_mm: f64,
        pixels: usize,
    ) -> CrossSectionResult {
        let n = self.n_cols();
        let idx = ((position_frac * (n - 1) as f64).round() as usize).min(n - 1);

        let (rot_normals, rot_binormals) = self.rotated_frame(rotation_deg);

        let pos = Vector3::new(
            self.positions[idx][0],
            self.positions[idx][1],
            self.positions[idx][2],
        );
        let n_vec = rot_normals[idx];
        let b_vec = rot_binormals[idx];
        let arc_mm = self.arclengths[idx];

        let inv_spacing = [1.0 / spacing[0], 1.0 / spacing[1], 1.0 / spacing[2]];

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
    }

    /// Render curved CPR using pixel-driven approach: for each output pixel,
    /// find the nearest projected centerline point and sample the 3D volume
    /// at the corresponding position offset by the signed perpendicular distance.
    ///
    /// This eliminates gaps and fan artifacts from the old centerline-driven
    /// strip-painting approach.
    pub fn render_curved_cpr(
        &self,
        volume: &Array3<f32>,
        spacing: [f64; 3],
        origin: [f64; 3],
        rotation_deg: f64,
        width_mm: f64,
        pixels_wide: usize,
        pixels_high: usize,
        slab_mm: f64,
    ) -> CurvedCprResult {
        let n_cols = self.n_cols();
        let (rot_normals, rot_binormals) = self.rotated_frame(rotation_deg);
        let inv_spacing = [1.0 / spacing[0], 1.0 / spacing[1], 1.0 / spacing[2]];

        // MIP slab sampling parameters
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

        // --- Project centerline onto a 2D viewing plane ---
        // The viewing plane is defined by the average binormal as the "into-screen"
        // direction. We compute a right/up basis from that.
        let avg_binormal = {
            let mut sum = Vector3::new(0.0, 0.0, 0.0);
            for b in &rot_binormals {
                sum += b;
            }
            sum / n_cols as f64
        };
        let view_in = avg_binormal.normalize();

        // view_right: perpendicular to view_in, preferring the horizontal plane
        let world_up = Vector3::new(1.0, 0.0, 0.0); // z-axis is "up" in [z,y,x] coords
        let view_right = {
            let candidate = world_up.cross(&view_in);
            if candidate.norm() > 1e-6 {
                candidate.normalize()
            } else {
                let fallback = Vector3::new(0.0, 1.0, 0.0);
                fallback.cross(&view_in).normalize()
            }
        };
        let view_up = view_in.cross(&view_right).normalize();

        // Project each centerline position onto the 2D plane
        let center_pos = Vector3::new(
            self.positions[n_cols / 2][0],
            self.positions[n_cols / 2][1],
            self.positions[n_cols / 2][2],
        );
        let projected: Vec<(f64, f64)> = self.positions.iter().map(|p| {
            let v = Vector3::new(p[0], p[1], p[2]) - center_pos;
            (v.dot(&view_right), v.dot(&view_up))
        }).collect();

        // Find bounding box of projected centerline + lateral extent
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
        // Add lateral padding
        min_x -= width_mm;
        max_x += width_mm;
        min_y -= width_mm;
        max_y += width_mm;

        let view_width = max_x - min_x;
        let view_height = max_y - min_y;

        // Scale factors: mm -> pixel
        let sx = if pixels_wide > 1 { (pixels_wide as f64 - 1.0) / view_width } else { 1.0 };
        let sy = if pixels_high > 1 { (pixels_high as f64 - 1.0) / view_height } else { 1.0 };

        let mut image = vec![f32::NAN; pixels_wide * pixels_high];

        // Width threshold squared for early rejection
        let width_sq = width_mm * width_mm;

        // Pixel-driven: for each output pixel, find nearest centerline point
        for row in 0..pixels_high {
            // Map pixel row to mm coordinate (flip y: top = max_y)
            let py_mm = max_y - row as f64 / sy;

            // Track best index from previous column for locality hint
            let mut hint_idx: usize = 0;

            for col in 0..pixels_wide {
                // Map pixel column to mm coordinate
                let px_mm = min_x + col as f64 / sx;

                // Find nearest projected centerline point (brute force over n_cols)
                // Start search from hint (previous best) and expand outward
                let mut best_idx = hint_idx;
                let dx0 = projected[hint_idx].0 - px_mm;
                let dy0 = projected[hint_idx].1 - py_mm;
                let mut best_dist_sq = dx0 * dx0 + dy0 * dy0;

                for k in 0..n_cols {
                    let dx = projected[k].0 - px_mm;
                    let dy = projected[k].1 - py_mm;
                    let d2 = dx * dx + dy * dy;
                    if d2 < best_dist_sq {
                        best_dist_sq = d2;
                        best_idx = k;
                    }
                }

                hint_idx = best_idx;

                // Skip if too far from centerline
                if best_dist_sq > width_sq {
                    continue;
                }

                // Compute signed perpendicular distance along local normal direction
                let centerline_2d = projected[best_idx];
                let delta = (px_mm - centerline_2d.0, py_mm - centerline_2d.1);

                // Project the 3D normal at best_idx into the 2D viewing plane
                let n_3d = rot_normals[best_idx];
                let n_proj_x = n_3d.dot(&view_right);
                let n_proj_y = n_3d.dot(&view_up);
                let n_len = (n_proj_x * n_proj_x + n_proj_y * n_proj_y).sqrt();
                if n_len < 1e-10 {
                    continue;
                }

                // Signed distance along the projected normal direction
                let signed_d = (delta.0 * n_proj_x + delta.1 * n_proj_y) / n_len;

                // Sample 3D volume: centerline position + signed_d * normal
                let pos = Vector3::new(
                    self.positions[best_idx][0],
                    self.positions[best_idx][1],
                    self.positions[best_idx][2],
                );
                let sample_mm = pos + signed_d * rot_normals[best_idx];

                // MIP slab along binormal
                let mut max_val = f32::NEG_INFINITY;
                for &slab_off in &slab_offsets {
                    let s = sample_mm + slab_off * rot_binormals[best_idx];
                    let vz = (s[0] - origin[0]) * inv_spacing[0];
                    let vy = (s[1] - origin[1]) * inv_spacing[1];
                    let vx = (s[2] - origin[2]) * inv_spacing[2];
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
            arclengths: self.arclengths.clone(),
        }
    }

    /// Batch render multiple cross-sections efficiently.
    pub fn render_cross_sections(
        &self,
        volume: &Array3<f32>,
        spacing: [f64; 3],
        origin: [f64; 3],
        position_fracs: &[f64],
        rotation_deg: f64,
        width_mm: f64,
        pixels: usize,
    ) -> Vec<CrossSectionResult> {
        let n = self.n_cols();
        let (rot_normals, rot_binormals) = self.rotated_frame(rotation_deg);
        let inv_spacing = [1.0 / spacing[0], 1.0 / spacing[1], 1.0 / spacing[2]];

        position_fracs
            .iter()
            .map(|&frac| {
                let idx = ((frac * (n - 1) as f64).round() as usize).min(n - 1);

                let pos = Vector3::new(
                    self.positions[idx][0],
                    self.positions[idx][1],
                    self.positions[idx][2],
                );
                let n_vec = rot_normals[idx];
                let b_vec = rot_binormals[idx];
                let arc_mm = self.arclengths[idx];

                let mut image = vec![f32::NAN; pixels * pixels];

                for row in 0..pixels {
                    for col in 0..pixels {
                        let offset_n = width_mm
                            * (1.0 - 2.0 * (row as f64) / ((pixels - 1) as f64));
                        let offset_b = width_mm
                            * (1.0 - 2.0 * (col as f64) / ((pixels - 1) as f64));

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

    // --- Internal helpers ---

    /// Apply rotation around the tangent axis, returning rotated (normals, binormals).
    /// Does NOT mutate self — returns new vectors for the requested angle.
    fn rotated_frame(
        &self,
        rotation_deg: f64,
    ) -> (Vec<Vector3<f64>>, Vec<Vector3<f64>>) {
        if rotation_deg.abs() < 1e-10 {
            return (self.normals.clone(), self.binormals.clone());
        }

        let theta = rotation_deg.to_radians();
        let cos_t = theta.cos();
        let sin_t = theta.sin();
        let n = self.normals.len();

        let mut rot_n = Vec::with_capacity(n);
        let mut rot_b = Vec::with_capacity(n);

        for i in 0..n {
            let ni = self.normals[i];
            let bi = self.binormals[i];
            rot_n.push(cos_t * ni + sin_t * bi);
            rot_b.push(-sin_t * ni + cos_t * bi);
        }

        (rot_n, rot_b)
    }
}

// ---------------------------------------------------------------------------
// Bishop frame (parallel transport)
// ---------------------------------------------------------------------------

/// Compute Rotation Minimizing Frame using the Double Reflection Method
/// (Wang et al. 2008). More numerically stable than simple projection-based
/// parallel transport — no error accumulation, even over long curves.
fn bishop_frame(tangents: &[Vector3<f64>]) -> (Vec<Vector3<f64>>, Vec<Vector3<f64>>) {
    let n = tangents.len();
    if n == 0 { return (vec![], vec![]); }

    let mut normals = Vec::with_capacity(n);
    let mut binormals = Vec::with_capacity(n);

    // Choose initial normal perpendicular to T[0]
    let t0 = tangents[0];
    let world_y = Vector3::new(0.0, 1.0, 0.0);
    let world_x = Vector3::new(1.0, 0.0, 0.0);
    let seed = if t0.cross(&world_y).norm() > 0.1 { world_y } else { world_x };
    let n0 = t0.cross(&seed).normalize();
    let b0 = t0.cross(&n0).normalize();
    normals.push(n0);
    binormals.push(b0);

    // Double Reflection Method
    for i in 0..n - 1 {
        let ti = tangents[i];
        let ti1 = tangents[i + 1];
        let ni = normals[i];

        // Reflection 1: across the bisector plane of T[i] and T[i+1]
        let v1 = ti1 - ti;
        let c1 = v1.dot(&v1);
        if c1 < 1e-30 {
            // Tangents are identical — frame doesn't change
            normals.push(ni);
            binormals.push(ti1.cross(&ni).normalize());
            continue;
        }
        let n_mid = ni - (2.0 / c1) * v1.dot(&ni) * v1;
        let t_mid = ti - (2.0 / c1) * v1.dot(&ti) * v1;

        // Reflection 2: across the plane perpendicular to T[i+1]
        let v2 = ti1 - t_mid;
        let c2 = v2.dot(&v2);
        let ni1 = if c2 < 1e-30 {
            n_mid
        } else {
            n_mid - (2.0 / c2) * v2.dot(&n_mid) * v2
        };

        // Normalize to prevent any drift (should be near unit already)
        let ni1 = ni1.normalize();
        let bi1 = ti1.cross(&ni1).normalize();
        normals.push(ni1);
        binormals.push(bi1);
    }

    (normals, binormals)
}

// ---------------------------------------------------------------------------
// Legacy public API wrappers (still used by existing tests)
// ---------------------------------------------------------------------------

/// Compute a Curved Planar Reformation (CPR) image from a volume along a
/// centerline. This is the single-call API (builds frame + renders in one shot).
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
    let frame = CprFrame::from_centerline(centerline_mm, pixels_wide);
    frame.render_cpr(volume, spacing, origin, rotation_deg, width_mm, pixels_high, slab_mm)
}

/// Compute a cross-sectional image perpendicular to the centerline at a
/// given arc-length position. Legacy single-call API.
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
    let n_samples = centerline_mm.len().max(2);
    let frame = CprFrame::from_centerline(centerline_mm, n_samples);
    frame.render_cross_section(volume, spacing, origin, position_frac, rotation_deg, width_mm, pixels)
}

/// Compute multiple cross-sections in a single call. Legacy single-call API.
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
    let n_samples = centerline_mm.len().max(2);
    let frame = CprFrame::from_centerline(centerline_mm, n_samples);
    frame.render_cross_sections(volume, spacing, origin, position_fracs, rotation_deg, width_mm, pixels)
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
    fn test_cpr_frame_cached_render() {
        // Use a volume with y-variation so rotation around the tangent axis
        // produces visibly different CPR images.
        let mut vol = Array3::<f32>::zeros((64, 64, 64));
        for z in 0..64 {
            for y in 0..64 {
                for x in 0..64 {
                    // value = z + y  (gradient along both z and y)
                    vol[[z, y, x]] = (z + y) as f32;
                }
            }
        }
        let spacing = [1.0, 1.0, 1.0];
        let origin = [0.0, 0.0, 0.0];

        let centerline: Vec<[f64; 3]> = (0..60)
            .map(|z| [z as f64, 32.0, 32.0])
            .collect();

        // Build frame once
        let frame = CprFrame::from_centerline(&centerline, 60);
        assert_eq!(frame.n_cols(), 60);
        assert_eq!(frame.arclengths.len(), 60);

        // Render at two different rotations — both should produce valid images
        let r1 = frame.render_cpr(&vol, spacing, origin, 0.0, 10.0, 21, 0.0);
        let r2 = frame.render_cpr(&vol, spacing, origin, 90.0, 10.0, 21, 0.0);

        assert_eq!(r1.image.len(), 60 * 21);
        assert_eq!(r2.image.len(), 60 * 21);

        // Images should differ (different rotation angles slice through y vs x gradient)
        let differs = r1.image.iter().zip(r2.image.iter())
            .any(|(a, b)| {
                !a.is_nan() && !b.is_nan() && (a - b).abs() > 0.01
            });
        assert!(differs, "different rotation angles should produce different images");
    }

    #[test]
    fn test_cpr_frame_cross_sections() {
        let (vol, spacing) = make_test_volume();
        let origin = [0.0, 0.0, 0.0];

        let centerline: Vec<[f64; 3]> = (0..60)
            .map(|z| [z as f64, 32.0, 32.0])
            .collect();

        let frame = CprFrame::from_centerline(&centerline, 60);

        // Batch cross-sections
        let results = frame.render_cross_sections(
            &vol, spacing, origin,
            &[0.25, 0.5, 0.75],
            0.0, 10.0, 21,
        );
        assert_eq!(results.len(), 3);

        // Arc-lengths should be increasing
        assert!(results[0].arc_mm < results[1].arc_mm);
        assert!(results[1].arc_mm < results[2].arc_mm);
    }
}
