use nalgebra::Vector3;
use ndarray::Array3;

use super::interp::trilinear;
use crate::pipeline::spline::CubicSpline3D;

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

    /// Render curved CPR: the vessel follows its natural projected path on screen.
    ///
    /// Instead of straightening the vessel into columns, each centerline position
    /// is projected onto a 2D viewing plane, and perpendicular strips are painted
    /// at the projected location.
    ///
    /// - `view_width_mm`, `view_height_mm`: physical size of the output viewport.
    /// - `pixels_wide`, `pixels_high`: output image dimensions.
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
        let (rot_normals, rot_binormals) = self.rotated_frame(rotation_deg);

        // Direct volume sampling with PCA viewing plane + 3D nearest-point lookup.
        // No texture warping — each pixel samples the volume directly.
        super::curved_cpr::render_curved_direct(
            &self.positions,
            &rot_normals,
            &rot_binormals,
            &self.arclengths,
            volume,
            spacing,
            origin,
            width_mm,
            pixels_wide,
            pixels_high,
            slab_mm,
            rotation_deg,
        )
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
    pub(crate) fn rotated_frame(
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

/// Compute Bishop (parallel-transport) frame along a sequence of tangent
/// vectors. Returns (normals, binormals).
fn bishop_frame(tangents: &[Vector3<f64>]) -> (Vec<Vector3<f64>>, Vec<Vector3<f64>>) {
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

    // -----------------------------------------------------------------------
    // Helper: quarter-circle centerline in Z-Y plane, radius r, n_pts points
    // Coordinates are [z, y, x]. Arc sweeps theta from 0 to pi/2.
    //   z = r * sin(theta),  y = r * cos(theta),  x = 0
    // -----------------------------------------------------------------------
    fn quarter_circle(r: f64, n_pts: usize) -> Vec<[f64; 3]> {
        (0..n_pts)
            .map(|i| {
                let theta = std::f64::consts::FRAC_PI_2 * (i as f64) / ((n_pts - 1) as f64);
                [r * theta.sin(), r * theta.cos(), 0.0]
            })
            .collect()
    }

    // Helper: S-curve — two quarter-circles curving in opposite directions.
    // First arc: center (0, r, 0), theta 0..pi/2  => z: 0..r,  y: r..0
    // Second arc: center (2r, 0, 0), theta pi..pi/2 => z: r..2r, y: 0..r
    // This produces an S-shape in the Z-Y plane.
    fn s_curve(r: f64, n_pts_per_arc: usize) -> Vec<[f64; 3]> {
        let mut pts = Vec::with_capacity(2 * n_pts_per_arc - 1);
        // First quarter-circle: curving from (0, r, 0) toward (r, 0, 0)
        for i in 0..n_pts_per_arc {
            let theta = std::f64::consts::FRAC_PI_2 * (i as f64) / ((n_pts_per_arc - 1) as f64);
            pts.push([r * theta.sin(), r * theta.cos(), 0.0]);
        }
        // Second quarter-circle: curving from (r, 0, 0) toward (2r, r, 0)
        // This reverses the curvature direction.
        for i in 1..n_pts_per_arc {
            let theta = std::f64::consts::FRAC_PI_2 * (i as f64) / ((n_pts_per_arc - 1) as f64);
            pts.push([r + r * theta.sin(), r * (1.0 - theta.cos()), 0.0]);
        }
        pts
    }

    #[test]
    fn test_bishop_frame_orthogonality() {
        let pts = quarter_circle(20.0, 20);
        let frame = CprFrame::from_centerline(&pts, 20);
        let tol = 1e-6;

        for i in 0..frame.n_cols() {
            let t = &frame.tangents[i];
            let n = &frame.normals[i];
            let b = &frame.binormals[i];

            let tn = t.dot(n).abs();
            let tb = t.dot(b).abs();
            let nb = n.dot(b).abs();

            assert!(
                tn < tol,
                "T.N not orthogonal at i={}: dot={:.2e}",
                i, tn
            );
            assert!(
                tb < tol,
                "T.B not orthogonal at i={}: dot={:.2e}",
                i, tb
            );
            assert!(
                nb < tol,
                "N.B not orthogonal at i={}: dot={:.2e}",
                i, nb
            );
        }
    }

    #[test]
    fn test_bishop_frame_unit_vectors() {
        let pts = quarter_circle(20.0, 20);
        let frame = CprFrame::from_centerline(&pts, 20);
        let tol = 1e-6;

        for i in 0..frame.n_cols() {
            let n_len = frame.normals[i].norm();
            let b_len = frame.binormals[i].norm();

            assert!(
                (n_len - 1.0).abs() < tol,
                "|N[{}]| = {:.10}, expected 1.0",
                i, n_len
            );
            assert!(
                (b_len - 1.0).abs() < tol,
                "|B[{}]| = {:.10}, expected 1.0",
                i, b_len
            );
        }
    }

    #[test]
    fn test_bishop_frame_right_hand_rule() {
        let pts = quarter_circle(20.0, 20);
        let frame = CprFrame::from_centerline(&pts, 20);
        let tol = 1e-6;

        for i in 0..frame.n_cols() {
            let t = &frame.tangents[i];
            let n = &frame.normals[i];
            let b = &frame.binormals[i];

            // B should equal T x N
            let cross = t.cross(n);
            let diff = (cross - b).norm();
            assert!(
                diff < tol,
                "B[{}] != T[{}] x N[{}]: diff norm = {:.2e}, B={:?}, TxN={:?}",
                i, i, i, diff, b, cross
            );
        }
    }

    #[test]
    fn test_bishop_frame_no_flip() {
        // Quarter-circle
        let pts = quarter_circle(20.0, 30);
        let frame = CprFrame::from_centerline(&pts, 30);

        for i in 0..frame.n_cols() - 1 {
            let dot = frame.normals[i].dot(&frame.normals[i + 1]);
            assert!(
                dot > 0.0,
                "Normal flip detected (quarter-circle) at i={}: N[i].N[i+1] = {:.6}",
                i, dot
            );
        }

        // S-curve
        let s_pts = s_curve(20.0, 20);
        let s_frame = CprFrame::from_centerline(&s_pts, 40);

        for i in 0..s_frame.n_cols() - 1 {
            let dot = s_frame.normals[i].dot(&s_frame.normals[i + 1]);
            assert!(
                dot > 0.0,
                "Normal flip detected (S-curve) at i={}: N[i].N[i+1] = {:.6}",
                i, dot
            );
        }
    }

    #[test]
    fn test_bishop_frame_smoothness() {
        // Quarter-circle with 50+ samples — consecutive normals should differ by < 15 deg
        let pts = quarter_circle(20.0, 60);
        let frame = CprFrame::from_centerline(&pts, 60);
        let max_angle_rad = 15.0_f64.to_radians();

        for i in 0..frame.n_cols() - 1 {
            let dot = frame.normals[i]
                .dot(&frame.normals[i + 1])
                .clamp(-1.0, 1.0);
            let angle = dot.acos();
            assert!(
                angle < max_angle_rad,
                "Angle between N[{}] and N[{}] = {:.2} deg, exceeds 15 deg",
                i,
                i + 1,
                angle.to_degrees()
            );
        }
    }

    #[test]
    fn test_frame_straight_line_constant() {
        // Straight line along Z at (y=32, x=32)
        let pts: Vec<[f64; 3]> = (0..60).map(|z| [z as f64, 32.0, 32.0]).collect();
        let frame = CprFrame::from_centerline(&pts, 60);
        let tol = 1e-6;

        let n0 = &frame.normals[0];
        let b0 = &frame.binormals[0];

        for i in 1..frame.n_cols() {
            let n_dot = frame.normals[i].dot(n0);
            let b_dot = frame.binormals[i].dot(b0);

            assert!(
                (n_dot - 1.0).abs() < tol,
                "N[{}] not parallel to N[0]: dot = {:.10}",
                i, n_dot
            );
            assert!(
                (b_dot - 1.0).abs() < tol,
                "B[{}] not parallel to B[0]: dot = {:.10}",
                i, b_dot
            );
        }
    }

    #[test]
    fn test_from_centerline_endpoints() {
        let pts = quarter_circle(20.0, 25);
        let frame = CprFrame::from_centerline(&pts, 50);
        let tol = 1e-3;

        let first = &frame.positions[0];
        let last = &frame.positions[frame.n_cols() - 1];

        let first_in = &pts[0];
        let last_in = &pts[pts.len() - 1];

        for d in 0..3 {
            assert!(
                (first[d] - first_in[d]).abs() < tol,
                "First position mismatch in dim {}: got {}, expected {}",
                d, first[d], first_in[d]
            );
            assert!(
                (last[d] - last_in[d]).abs() < tol,
                "Last position mismatch in dim {}: got {}, expected {}",
                d, last[d], last_in[d]
            );
        }
    }

    #[test]
    fn test_from_centerline_uniform_spacing() {
        let pts = quarter_circle(20.0, 30);
        let frame = CprFrame::from_centerline(&pts, 100);

        let n = frame.arclengths.len();
        assert_eq!(n, 100);

        // Compute spacings
        let spacings: Vec<f64> = (0..n - 1)
            .map(|i| frame.arclengths[i + 1] - frame.arclengths[i])
            .collect();

        let mean_spacing: f64 = spacings.iter().sum::<f64>() / spacings.len() as f64;
        assert!(mean_spacing > 0.0, "Mean spacing must be positive");

        for (i, &ds) in spacings.iter().enumerate() {
            let rel_err = ((ds - mean_spacing) / mean_spacing).abs();
            assert!(
                rel_err < 0.01,
                "Non-uniform spacing at i={}: ds={:.8}, mean={:.8}, rel_err={:.4}%",
                i, ds, mean_spacing, rel_err * 100.0
            );
        }
    }

    /// Test every 1° rotation angle on real patient data.
    /// Verifies the centerline pixels show high HU (inside vessel) at all angles.
    #[test]
    #[ignore] // cargo test --lib -- --ignored --nocapture test_curved_cpr_centerline_hu_real_patient
    fn test_curved_cpr_centerline_hu_real_patient() {
        use std::path::Path;
        use super::super::curved_cpr;

        // Patient 317.6 — the one the user reported the issue on
        let dicom_dir = Path::new("/Users/shunie/Developer/PCAT/Rahaf_Patients/317.6");
        if !dicom_dir.exists() {
            eprintln!("DICOM dir not found, skipping");
            return;
        }

        // Seeds from saved file (cornerstone [x, y, z] ordering → convert to [z, y, x])
        let seeds_xyz: Vec<[f64; 3]> = vec![
            [18.513, -174.185, 1922.507],
            [18.071, -181.259, 1922.507],
            [14.976, -187.007, 1922.507],
            [12.765, -190.987, 1922.507],
            [2.596, -194.966, 1915.733],
            [-2.268, -198.061, 1910.314],
            [-5.500, -198.061, 1904.611],
            [-8.599, -198.061, 1896.691],
            [-10.321, -198.061, 1890.838],
            [-10.665, -193.840, 1879.820],
            [-8.255, -190.791, 1875.0],
            [-0.576, -182.322, 1868.712],
            [6.695, -174.192, 1868.051],
            [10.881, -169.450, 1868.932],
            [17.051, -162.336, 1872.017],
            [22.119, -155.561, 1875.762],
        ];
        // Convert to [z, y, x]
        let seeds_zyx: Vec<[f64; 3]> = seeds_xyz.iter()
            .map(|s| [s[2], s[1], s[0]])
            .collect();

        eprintln!("Loading volume...");
        let vol = crate::pipeline::dicom_loader::load_dicom_directory(dicom_dir).unwrap();
        eprintln!("Volume: {:?}, spacing: {:?}, origin: {:?}", vol.data.shape(), vol.spacing, vol.origin);

        // Build spline + frame
        let spline = crate::pipeline::spline::CubicSpline3D::fit(&seeds_zyx);
        let n_cols = 512;
        let cl: Vec<[f64; 3]> = (0..n_cols)
            .map(|i| spline.eval(spline.total_arc() * i as f64 / (n_cols - 1) as f64))
            .collect();
        let frame = CprFrame::from_centerline(&cl, n_cols);

        let width_mm = 25.0;
        let px_w = 512;
        let px_h = 256;
        let hu_threshold = 150.0; // iodinated blood should be >200 HU

        let mut failed_angles: Vec<(i32, f64, usize, usize)> = Vec::new();
        let mut all_worst: Vec<(i32, f64)> = Vec::new();

        for rot_deg in 0..360 {
            let result = frame.render_curved_cpr(
                &vol.data, vol.spacing, vol.origin,
                rot_deg as f64, width_mm, px_w, px_h, 1.0,
            );

            // Project centerline to pixel coordinates (PCA-based, matches renderer)
            let (_vf, vr, vu) = curved_cpr::compute_view_basis_pca_with_rotation(
                &frame.positions, rot_deg as f64,
            );
            let mid_idx = frame.n_cols() / 2;
            let center = frame.positions[mid_idx];
            let projected = curved_cpr::project_centerline_2d(
                &frame.positions, center, &vr, &vu,
            );

            // Bbox (same as renderer)
            let mut bmin_x = f64::MAX;
            let mut bmax_x = f64::NEG_INFINITY;
            let mut bmin_y = f64::MAX;
            let mut bmax_y = f64::NEG_INFINITY;
            for &(px, py) in &projected {
                if px < bmin_x { bmin_x = px; }
                if px > bmax_x { bmax_x = px; }
                if py < bmin_y { bmin_y = py; }
                if py > bmax_y { bmax_y = py; }
            }
            let pad = curved_cpr::CONTEXT_PAD_MM;
            bmin_x -= pad; bmax_x += pad;
            bmin_y -= pad; bmax_y += pad;
            // Isotropic correction (same as renderer)
            let mut vw = bmax_x - bmin_x;
            let mut vh = bmax_y - bmin_y;
            let target_ratio = px_w as f64 / px_h as f64;
            let bbox_ratio = vw / vh;
            if bbox_ratio < target_ratio {
                let new_w = vh * target_ratio;
                let extra = (new_w - vw) / 2.0;
                bmin_x -= extra; bmax_x += extra;
                vw = bmax_x - bmin_x;
            } else {
                let new_h = vw / target_ratio;
                let extra = (new_h - vh) / 2.0;
                bmin_y -= extra; bmax_y += extra;
                vh = bmax_y - bmin_y;
            }

            // Check middle 80% of centerline
            let n = frame.n_cols();
            let start = n / 10;
            let end = n - n / 10;
            let mut worst_hu = f64::MAX;
            let mut n_checked = 0usize;
            let mut n_low = 0usize;

            for j in start..end {
                let (px, py) = projected[j];
                let col = ((px - bmin_x) / vw * (px_w - 1) as f64).round() as isize;
                let row = ((bmax_y - py) / vh * (px_h - 1) as f64).round() as isize;
                if col < 1 || col >= (px_w - 1) as isize || row < 1 || row >= (px_h - 1) as isize {
                    continue;
                }

                // Max value in 5x5 neighbourhood (vessel is a few pixels wide)
                let mut best_val = f32::NEG_INFINITY;
                for dr in -2..=2isize {
                    for dc in -2..=2isize {
                        let r = (row + dr).max(0).min(px_h as isize - 1) as usize;
                        let c = (col + dc).max(0).min(px_w as isize - 1) as usize;
                        let val = result.image[r * px_w + c];
                        if !val.is_nan() && val > best_val {
                            best_val = val;
                        }
                    }
                }
                n_checked += 1;
                let hu = best_val as f64;
                if hu < worst_hu { worst_hu = hu; }
                if hu < hu_threshold { n_low += 1; }
            }

            all_worst.push((rot_deg, worst_hu));

            // More than 15% of centerline points below threshold → failure
            if n_checked > 0 && (n_low as f64 / n_checked as f64) > 0.15 {
                failed_angles.push((rot_deg, worst_hu, n_checked, n_low));
            }
        }

        // Print summary
        eprintln!("\n=== Worst HU at centerline per angle (first 36) ===");
        for chunk in all_worst.chunks(36) {
            let line: Vec<String> = chunk.iter()
                .map(|(a, hu)| format!("{}°:{:.0}", a, hu))
                .collect();
            eprintln!("  {}", line.join("  "));
        }

        if !failed_angles.is_empty() {
            eprintln!("\n=== FAILED ANGLES ({}) ===", failed_angles.len());
            for (a, hu, nc, nl) in &failed_angles {
                eprintln!("  {}° — worst HU={:.0}, {}/{} low (<{:.0})", a, hu, nl, nc, hu_threshold);
            }
            panic!(
                "Vessel centerline below {} HU at {} out of 360 angles",
                hu_threshold, failed_angles.len()
            );
        }
        eprintln!("\nAll 360 angles passed: centerline always inside vessel (>{} HU)", hu_threshold);
    }

    /// Test patient 161.6 — the patient where seeds at 90°/270° failed.
    #[test]
    #[ignore]
    fn test_curved_cpr_patient_161() {
        use std::path::Path;
        use super::super::curved_cpr;

        let dicom_dir = Path::new("/Users/shunie/Developer/PCAT/Rahaf_Patients/161.6/CCTA l-70 (KVP)");
        if !dicom_dir.exists() { eprintln!("DICOM dir not found"); return; }

        // Seeds from saved file (cornerstone [x, y, z] → [z, y, x])
        let seeds_xyz: Vec<[f64; 3]> = vec![
            [-10.87,-224.30,1744.50], [-3.39,-231.24,1744.50],
            [0.87,-238.71,1744.50],   [4.08,-245.12,1744.50],
            [2.22,-249.07,1740.90],   [-2.33,-250.50,1740.90],
            [-9.03,-251.46,1740.90],  [-13.58,-251.70,1740.90],
            [-16.78,-251.70,1738.14], [-19.09,-251.70,1732.53],
            [-20.08,-252.98,1727.25], [-22.72,-255.00,1720.66],
            [-27.01,-257.84,1715.05], [-30.64,-257.84,1711.09],
            [-32.95,-257.84,1706.47], [-34.27,-252.57,1696.24],
            [-34.27,-250.37,1690.64], [-34.27,-249.38,1685.36],
            [-27.43,-249.71,1676.78], [-17.70,-237.50,1680.08],
            [-10.41,-226.62,1682.39], [-6.76,-222.33,1683.71],
            [0.13,-217.71,1685.03],
        ];
        let seeds_zyx: Vec<[f64; 3]> = seeds_xyz.iter().map(|s| [s[2], s[1], s[0]]).collect();

        eprintln!("Loading 161.6 volume...");
        let vol = crate::pipeline::dicom_loader::load_dicom_directory(dicom_dir).unwrap();
        eprintln!("Volume: {:?}, spacing: {:?}", vol.data.shape(), vol.spacing);

        let spline = crate::pipeline::spline::CubicSpline3D::fit(&seeds_zyx);
        let n_cols = 512;
        let cl: Vec<[f64; 3]> = (0..n_cols)
            .map(|i| spline.eval(spline.total_arc() * i as f64 / (n_cols - 1) as f64))
            .collect();
        let frame = CprFrame::from_centerline(&cl, n_cols);

        let px_w = 512usize;
        let px_h = 512usize;
        let width_mm = 25.0;
        let hu_threshold = 150.0;

        let mut failed_angles: Vec<(i32, f64, usize, usize)> = Vec::new();
        let mut all_worst: Vec<(i32, f64)> = Vec::new();

        for rot_deg in 0..360 {
            let result = frame.render_curved_cpr(
                &vol.data, vol.spacing, vol.origin,
                rot_deg as f64, width_mm, px_w, px_h, 1.0,
            );
            let (_vf, vr, vu) = curved_cpr::compute_view_basis_pca_with_rotation(
                &frame.positions, rot_deg as f64,
            );
            let mid_idx = frame.n_cols() / 2;
            let center = frame.positions[mid_idx];
            let projected = curved_cpr::project_centerline_2d(&frame.positions, center, &vr, &vu);

            let mut bmin_x = f64::MAX; let mut bmax_x = f64::NEG_INFINITY;
            let mut bmin_y = f64::MAX; let mut bmax_y = f64::NEG_INFINITY;
            for &(px, py) in &projected {
                if px < bmin_x { bmin_x = px; }
                if px > bmax_x { bmax_x = px; }
                if py < bmin_y { bmin_y = py; }
                if py > bmax_y { bmax_y = py; }
            }
            let pad = curved_cpr::CONTEXT_PAD_MM;
            bmin_x -= pad; bmax_x += pad; bmin_y -= pad; bmax_y += pad;
            let mut vw = bmax_x - bmin_x;
            let mut vh = bmax_y - bmin_y;
            let target_ratio = px_w as f64 / px_h as f64;
            let bbox_ratio = vw / vh;
            if bbox_ratio < target_ratio {
                let extra = (vh * target_ratio - vw) / 2.0;
                bmin_x -= extra; bmax_x += extra; vw = bmax_x - bmin_x;
            } else {
                let extra = (vw / target_ratio - vh) / 2.0;
                bmin_y -= extra; bmax_y += extra; vh = bmax_y - bmin_y;
            }

            let n = frame.n_cols();
            let start = n / 10; let end = n - n / 10;
            let mut worst_hu = f64::MAX;
            let mut n_checked = 0usize; let mut n_low = 0usize;

            for j in start..end {
                let (px, py) = projected[j];
                let col = ((px - bmin_x) / vw * (px_w - 1) as f64).round() as isize;
                let row = ((bmax_y - py) / vh * (px_h - 1) as f64).round() as isize;
                if col < 2 || col >= (px_w - 2) as isize || row < 2 || row >= (px_h - 2) as isize { continue; }
                let mut best_val = f32::NEG_INFINITY;
                for dr in -2..=2isize {
                    for dc in -2..=2isize {
                        let r = (row + dr) as usize;
                        let c = (col + dc) as usize;
                        let val = result.image[r * px_w + c];
                        if !val.is_nan() && val > best_val { best_val = val; }
                    }
                }
                n_checked += 1;
                let hu = best_val as f64;
                if hu < worst_hu { worst_hu = hu; }
                if hu < hu_threshold { n_low += 1; }
            }
            all_worst.push((rot_deg, worst_hu));
            if n_checked > 0 && (n_low as f64 / n_checked as f64) > 0.15 {
                failed_angles.push((rot_deg, worst_hu, n_checked, n_low));
            }
        }

        // Print key angles
        for &deg in &[0, 45, 90, 135, 180, 225, 270, 315] {
            let (_, hu) = all_worst[deg];
            eprintln!("  {}°: worst HU = {:.0}", deg, hu);
        }

        if !failed_angles.is_empty() {
            eprintln!("\n=== FAILED ({}) ===", failed_angles.len());
            for (a, hu, nc, nl) in &failed_angles {
                eprintln!("  {}° — worst={:.0}, {}/{} low", a, hu, nl, nc);
            }
            panic!("Failed at {} angles", failed_angles.len());
        }
        eprintln!("\nAll 360 angles passed (>{} HU)", hu_threshold);
    }

    #[test]
    #[ignore] // cargo test --lib -- --ignored --nocapture test_rca_reference
    fn test_rca_reference() {
        use std::path::Path;

        let dicom_dir = Path::new("/Users/shunie/Developer/PCAT/Rahaf_Patients/1200.2");
        if !dicom_dir.exists() {
            eprintln!("DICOM dir not found, skipping");
            return;
        }

        // Saved RCA seeds [x,y,z] → [z,y,x]
        let seeds_zyx: Vec<[f64; 3]> = vec![
            [1844.686, -177.084, 44.949], [1844.686, -183.413, 43.262],
            [1844.686, -188.476, 40.730], [1844.686, -191.852, 36.089],
            [1843.425, -192.696, 31.248], [1842.424, -193.118, 23.574],
            [1842.424, -192.696, 17.235], [1840.923, -194.942, 12.564],
            [1836.752, -194.942, 9.895],  [1830.079, -195.609, 10.228],
            [1824.741, -194.942, 12.230], [1819.403, -194.942, 14.232],
            [1814.898, -192.940, 15.900], [1811.729, -192.021, 18.569],
            [1807.725, -188.780, 20.238], [1805.390, -180.677, 24.241],
            [1806.724, -168.362, 28.579], [1806.391, -157.342, 37.587],
            [1806.724, -150.861, 45.928], [1808.057, -148.813, 48.747],
            [1811.395, -145.027, 49.932], [1818.068, -139.517, 54.603],
            [1821.404, -139.517, 58.940], [1822.406, -134.332, 65.947],
        ];

        let vol = crate::pipeline::dicom_loader::load_dicom_directory(dicom_dir).unwrap();
        eprintln!("Volume: {:?}, spacing: {:?}", vol.data.shape(), vol.spacing);

        let spline = crate::pipeline::spline::CubicSpline3D::fit(&seeds_zyx);
        let n = 768;
        let cl: Vec<[f64; 3]> = (0..n)
            .map(|i| spline.eval(spline.total_arc() * i as f64 / (n - 1) as f64))
            .collect();

        let frame = CprFrame::from_centerline(&cl, n);
        let out_dir = Path::new("/Users/shunie/Developer/PCAT/pcat-workstation-v2/test_output");
        std::fs::create_dir_all(out_dir).unwrap();

        // Test with width=25 (50mm total, matching syngo.via FOV)
        for rot in [0.0, 90.0, 180.0, 270.0] {
            let result = frame.render_curved_cpr(
                &vol.data, vol.spacing, vol.origin, rot, 25.0, 768, 384, 1.0,
            );
            let valid: Vec<f32> = result.image.iter().copied().filter(|v| !v.is_nan()).collect();
            let nan_pct = 100.0 * (result.image.len() - valid.len()) as f64 / result.image.len() as f64;
            eprintln!("Rot {:.0}°: {:.1}% NaN, range [{:.0}, {:.0}]",
                rot, nan_pct,
                valid.iter().copied().fold(f32::INFINITY, f32::min),
                valid.iter().copied().fold(f32::NEG_INFINITY, f32::max),
            );
            let bytes: &[u8] = bytemuck::cast_slice(&result.image);
            std::fs::write(out_dir.join(format!("rca_curved_rot{:.0}.raw", rot)), bytes).unwrap();
        }

        // Also straightened
        let straight = frame.render_cpr(&vol.data, vol.spacing, vol.origin, 0.0, 40.0, 384, 1.0);
        let bytes: &[u8] = bytemuck::cast_slice(&straight.image);
        std::fs::write(out_dir.join("rca_straightened.raw"), bytes).unwrap();
        eprintln!("Straightened: {}x{}", straight.pixels_wide, straight.pixels_high);

        eprintln!("Output saved to {:?}", out_dir);
    }
}
