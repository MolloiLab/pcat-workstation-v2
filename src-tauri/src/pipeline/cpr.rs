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
    /// 2D pixel coordinates of each centerline position in the output image.
    /// Length = n_cols, each entry is (pixel_col, pixel_row).
    pub projected_pixels: Vec<(f64, f64)>,
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
pub struct CprFrame {
    pub positions: Vec<[f64; 3]>,    // [z,y,x] in mm, n_cols entries
    pub tangents: Vec<Vector3<f64>>,
    pub normals: Vec<Vector3<f64>>,  // Bishop frame (parallel transport)
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

        fill_nan(&mut image, -1024.0);

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

    /// Render curved CPR using Horos "stretched" mode with fixed projection normal.
    ///
    /// Instead of using rotating frame normals per column (which creates fan artifacts),
    /// this uses a FIXED projection direction -- the average binormal as the "into-screen"
    /// direction. Every column samples in the same physical direction.
    ///
    /// Pixel-driven: for each output pixel, find the nearest centerline point and
    /// sample the volume at the corresponding 3D position.
    ///
    /// Returns projected pixel coordinates of each centerline point for overlay drawing.
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

        // Horos "stretched" mode: use a FIXED projection normal.
        // The projection normal is the average binormal direction (into-screen).
        let avg_binormal = {
            let sum: Vector3<f64> = rot_binormals.iter().copied().fold(
                Vector3::new(0.0, 0.0, 0.0),
                |acc, b| acc + b,
            );
            (sum / n_cols as f64).normalize()
        };

        // The "up" direction in the output image is FIXED (not rotating).
        // Choose a direction perpendicular to avg_binormal that's most "vertical".
        let world_z = Vector3::new(1.0, 0.0, 0.0); // z-up in [z,y,x] coords
        let fixed_up = {
            let projected = world_z - world_z.dot(&avg_binormal) * avg_binormal;
            if projected.norm() > 0.1 {
                projected.normalize()
            } else {
                let fallback = Vector3::new(0.0, 1.0, 0.0);
                (fallback - fallback.dot(&avg_binormal) * avg_binormal).normalize()
            }
        };

        // "right" direction: perpendicular to both
        let fixed_right = avg_binormal.cross(&fixed_up).normalize();

        // Project each centerline position onto the 2D plane (right, up)
        let center_pos = Vector3::new(
            self.positions[n_cols / 2][0],
            self.positions[n_cols / 2][1],
            self.positions[n_cols / 2][2],
        );
        let projected: Vec<(f64, f64)> = self.positions.iter().map(|p| {
            let v = Vector3::new(p[0], p[1], p[2]) - center_pos;
            (v.dot(&fixed_right), v.dot(&fixed_up))
        }).collect();

        // Bounding box with padding
        let (mut min_x, mut max_x, mut min_y, mut max_y) =
            (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
        for &(px, py) in &projected {
            min_x = min_x.min(px);
            max_x = max_x.max(px);
            min_y = min_y.min(py);
            max_y = max_y.max(py);
        }
        min_x -= width_mm;
        max_x += width_mm;
        min_y -= width_mm;
        max_y += width_mm;

        let view_w = max_x - min_x;
        let view_h = max_y - min_y;
        let sx = (pixels_wide as f64 - 1.0) / view_w;
        let sy = (pixels_high as f64 - 1.0) / view_h;

        // MIP slab
        let n_slab = if slab_mm > 0.01 { 5usize } else { 1 };
        let slab_offsets: Vec<f64> = if n_slab > 1 {
            (0..n_slab)
                .map(|k| -slab_mm / 2.0 + slab_mm * (k as f64) / ((n_slab - 1) as f64))
                .collect()
        } else {
            vec![0.0]
        };

        let mut image = vec![f32::NAN; pixels_wide * pixels_high];

        // PIXEL-DRIVEN: for each output pixel, find nearest centerline point
        for row in 0..pixels_high {
            let py_mm = max_y - (row as f64) / sy;

            // Track best index from previous column for locality
            let mut hint = 0usize;

            for col in 0..pixels_wide {
                let px_mm = min_x + (col as f64) / sx;

                // Find nearest projected centerline point (with locality hint)
                let mut best_idx = hint;
                let mut best_d2 = f64::MAX;

                // Search around the hint first
                let search_start = if hint > 30 { hint - 30 } else { 0 };
                let search_end = (hint + 30).min(n_cols);
                for k in search_start..search_end {
                    let dx = projected[k].0 - px_mm;
                    let dy = projected[k].1 - py_mm;
                    let d2 = dx * dx + dy * dy;
                    if d2 < best_d2 {
                        best_d2 = d2;
                        best_idx = k;
                    }
                }
                // If the local search didn't find something close, search globally
                if best_d2 > (width_mm * 1.5) * (width_mm * 1.5) {
                    for k in 0..n_cols {
                        let dx = projected[k].0 - px_mm;
                        let dy = projected[k].1 - py_mm;
                        let d2 = dx * dx + dy * dy;
                        if d2 < best_d2 {
                            best_d2 = d2;
                            best_idx = k;
                        }
                    }
                }

                hint = best_idx;
                let dist = best_d2.sqrt();
                if dist > width_mm {
                    continue;
                }

                // KEY DIFFERENCE from strip-painting:
                // The perpendicular distance from centerline in the fixed_up direction
                let centerline_3d = Vector3::new(
                    self.positions[best_idx][0],
                    self.positions[best_idx][1],
                    self.positions[best_idx][2],
                );
                let pixel_offset_mm = py_mm - projected[best_idx].1;

                // Sample 3D: centerline + offset in the per-column normal direction
                let sample_base = centerline_3d + pixel_offset_mm * rot_normals[best_idx];

                // MIP slab along binormal
                let mut max_val = f32::NEG_INFINITY;
                for &slab_off in &slab_offsets {
                    let s = sample_base + slab_off * rot_binormals[best_idx];
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

        fill_nan(&mut image, -1024.0);

        // Compute projected pixel coordinates for each centerline point (for overlays)
        let projected_pixels: Vec<(f64, f64)> = projected.iter().map(|&(px, py)| {
            let pixel_col = (px - min_x) * sx;
            let pixel_row = (max_y - py) * sy;
            (pixel_col, pixel_row)
        }).collect();

        CurvedCprResult {
            image,
            pixels_wide,
            pixels_high,
            arclengths: self.arclengths.clone(),
            projected_pixels,
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

/// Horos-style Rotation Minimizing Frame using axis-angle rotation.
/// Instead of projecting the previous normal (which accumulates drift),
/// compute the rotation that maps T[i] to T[i+1] and apply that same
/// rotation to N[i]. Then re-project to ensure strict orthogonality.
///
/// Based on N3VectorBend() from Horos/Nitrogen/Sources/N3Geometry.m
fn bishop_frame(tangents: &[Vector3<f64>]) -> (Vec<Vector3<f64>>, Vec<Vector3<f64>>) {
    let n = tangents.len();
    if n == 0 {
        return (vec![], vec![]);
    }

    let mut normals = Vec::with_capacity(n);
    let mut binormals = Vec::with_capacity(n);

    // Initial normal: perpendicular to T[0]
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

    for i in 0..n - 1 {
        let ti = tangents[i];
        let ti1 = tangents[i + 1];
        let ni = normals[i];

        // Horos N3VectorBend: rotate ni by the same rotation that maps ti -> ti1
        let bent = vector_bend(ni, ti, ti1);

        // Re-project to ensure strict orthogonality (Horos does this too)
        let projected = bent - bent.dot(&ti1) * ti1;
        let ni1 = if projected.norm() > 1e-12 {
            projected.normalize()
        } else {
            // Degenerate -- re-seed
            let s = if ti1.cross(&world_y).norm() > 0.1 {
                world_y
            } else {
                world_x
            };
            ti1.cross(&s).normalize()
        };
        let bi1 = ti1.cross(&ni1).normalize();
        normals.push(ni1);
        binormals.push(bi1);
    }

    (normals, binormals)
}

/// Port of Horos N3VectorBend (N3Geometry.m line 333).
/// Computes the minimum rotation that maps `original_dir` to `new_dir`
/// and applies that same rotation to `vector_to_bend`.
fn vector_bend(
    vector_to_bend: Vector3<f64>,
    original_dir: Vector3<f64>,
    new_dir: Vector3<f64>,
) -> Vector3<f64> {
    let orig_n = original_dir.normalize();
    let new_n = new_dir.normalize();

    let rotation_axis = orig_n.cross(&new_n);
    let axis_len = rotation_axis.norm();

    if axis_len < 1e-15 {
        // Directions are parallel (or anti-parallel)
        if orig_n.dot(&new_n) >= 0.0 {
            return vector_to_bend; // same direction, no rotation
        } else {
            // 180-degree rotation -- pick any perpendicular axis
            let perp = if orig_n.cross(&Vector3::new(1.0, 0.0, 0.0)).norm() > 0.1 {
                orig_n.cross(&Vector3::new(1.0, 0.0, 0.0)).normalize()
            } else {
                orig_n.cross(&Vector3::new(0.0, 1.0, 0.0)).normalize()
            };
            // Rotate 180 degrees around perp: v -> 2*(v.perp)*perp - v
            return 2.0 * vector_to_bend.dot(&perp) * perp - vector_to_bend;
        }
    }

    // Compute angle using asin (matching Horos)
    let sin_angle = axis_len.min(1.0); // clamp for numerical safety
    let mut angle = sin_angle.asin();
    if orig_n.dot(&new_n) < 0.0 {
        angle = std::f64::consts::PI - angle;
    }

    // Apply axis-angle rotation using Rodrigues' formula:
    // v_rot = v*cos(a) + (k x v)*sin(a) + k*(k.v)*(1-cos(a))
    let k = rotation_axis.normalize();
    let cos_a = angle.cos();
    let sin_a = angle.sin();

    vector_to_bend * cos_a
        + k.cross(&vector_to_bend) * sin_a
        + k * k.dot(&vector_to_bend) * (1.0 - cos_a)
}

/// Fill NaN pixels with a constant value. Horos uses nearest-neighbor fill,
/// but simple constant fill is faster and sufficient for display.
fn fill_nan(image: &mut [f32], fill_value: f32) {
    for v in image.iter_mut() {
        if v.is_nan() {
            *v = fill_value;
        }
    }
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
