# Curved CPR Quality: Wider FOV + Artifact-Free Rendering

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Match syngo.via curved CPR quality — full anatomical context (bones, chambers), no seam artifacts, smooth tissue at all rotation angles.

**Architecture:** Two-layer composite renderer. Layer 1: oblique slab MIP through the PCA best-fit plane (clean context everywhere — bones, chambers, fat). Layer 2: CPR sampling near the vessel (within ±25mm) for accurate vessel cross-section. Blend smoothly at the boundary. The oblique layer has zero artifacts because it samples the viewing plane directly. The CPR layer is only used where the vessel assignment is unambiguous.

**Tech Stack:** Rust (pipeline/curved_cpr.rs), nalgebra, ndarray

---

## Root Cause Analysis

Looking at the syngo.via reference side-by-side with our output, three problems remain:

### Problem 1: Narrow FOV
The bounding box is padded by `width_mm` (25mm) around the projected centerline. Syngo.via shows the entire heart — bones, spine, chambers — which extends ~80-100mm from the centerline. Our output clips to ±25mm, showing only tissue immediately around the vessel.

**Fix:** Pad bounding box by a much larger value (e.g., 80mm) independent of `width_mm`.

### Problem 2: Seam line at Voronoi boundaries
When the 3D curve wraps around, two distant centerline segments can be equidistant from a pixel. The nearest-segment assignment switches abruptly, creating a visible seam (different normals → different anatomy on each side).

**Fix:** Only use CPR sampling when the nearest-segment is UNAMBIGUOUS (far from any Voronoi boundary). At boundaries, fall back to oblique sampling which has no segment dependency.

### Problem 3: Abrupt CPR-to-oblique transition
Currently the code switches from `interp_pos + lateral * normal` to `pixel_3d` at exactly `lateral == width_mm`. This creates a visible edge. Also, the oblique path still uses `interp_binormal` for slab direction, which varies per-segment and causes texture jumps.

**Fix:** Use a smooth blend zone (5mm) between CPR and oblique. For oblique sampling, use fixed `view_forward` as slab direction.

## File Structure

| File | Changes |
|------|---------|
| `src-tauri/src/pipeline/curved_cpr.rs` | Replace `render_curved_direct` with two-layer composite renderer |
| `src-tauri/src/pipeline/cpr.rs` | Update `render_curved_cpr` call signature, update reference test |
| `src/components/CprView.svelte` | No changes needed (width_mm stays 25, bounding box is internal) |

---

## Task 1: Refactor `render_curved_direct` to two-layer composite

**Files:**
- Modify: `src-tauri/src/pipeline/curved_cpr.rs:433-568` (the `render_curved_direct` function)

- [ ] **Step 1: Compute view_forward for oblique slab**

After computing `view_right` and `view_up` from binormals, compute `view_forward`:

```rust
let (view_forward, view_right, view_up) = compute_view_basis(binormals);
```

(Already returned but currently assigned to `_vf` — just use it.)

- [ ] **Step 2: Increase bounding box padding**

Change the bounding box padding from `width_mm` to a fixed `80.0mm`:

```rust
let context_pad_mm = 80.0; // show bones, chambers, full context
min_x -= context_pad_mm;
max_x += context_pad_mm;
min_y -= context_pad_mm;
max_y += context_pad_mm;
```

- [ ] **Step 3: Replace the per-pixel sampling with two-layer composite**

Replace the current per-pixel body (lines ~496-563) with:

```rust
for row in 0..pixels_high {
    for col in 0..pixels_wide {
        let x_mm = min_x + (col as f64) * view_width / ((pixels_wide - 1) as f64);
        let y_mm = max_y - (row as f64) * view_height / ((pixels_high - 1) as f64);
        let pixel_3d = center_vec + x_mm * view_right + y_mm * view_up;

        // --- Layer 1: Oblique slab MIP (always computed, no artifacts) ---
        let mut oblique_val = f32::NEG_INFINITY;
        for &slab_off in &slab_offsets {
            let s = pixel_3d + slab_off * view_forward;
            let vz = (s[0] - origin[0]) * inv_spacing[0];
            let vy = (s[1] - origin[1]) * inv_spacing[1];
            let vx = (s[2] - origin[2]) * inv_spacing[2];
            let val = trilinear(volume, vz, vy, vx);
            if !val.is_nan() && val > oblique_val {
                oblique_val = val;
            }
        }
        let oblique_val = if oblique_val == f32::NEG_INFINITY { f32::NAN } else { oblique_val };

        // --- Layer 2: CPR sampling (only near vessel) ---
        // Find nearest 3D segment
        let mut best_j = 0usize;
        let mut best_frac = 0.0f64;
        let mut best_dist_sq = f64::MAX;
        let mut second_dist_sq = f64::MAX;
        for j in 0..n - 1 {
            let ab = pos_vecs[j + 1] - pos_vecs[j];
            let ap = pixel_3d - pos_vecs[j];
            let ab_len_sq = ab.norm_squared();
            let t = if ab_len_sq > 1e-20 {
                ap.dot(&ab) / ab_len_sq
            } else {
                0.0
            }.clamp(0.0, 1.0);
            let closest = pos_vecs[j] + t * ab;
            let d = (pixel_3d - closest).norm_squared();
            if d < best_dist_sq {
                second_dist_sq = best_dist_sq;
                best_dist_sq = d;
                best_j = j;
                best_frac = t;
            } else if d < second_dist_sq {
                second_dist_sq = d;
            }
        }

        let best_dist = best_dist_sq.sqrt();

        // Interpolate frame at nearest segment
        let j1 = best_j + 1;
        let interp_pos = pos_vecs[best_j] + best_frac * (pos_vecs[j1] - pos_vecs[best_j]);
        let interp_normal = (normals[best_j] * (1.0 - best_frac) + normals[j1] * best_frac).normalize();
        let interp_binormal = (binormals[best_j] * (1.0 - best_frac) + binormals[j1] * best_frac).normalize();

        let offset = pixel_3d - interp_pos;
        let lateral = offset.dot(&interp_normal);

        // Voronoi ambiguity check: if second-nearest is close to nearest,
        // the segment assignment is ambiguous → don't trust CPR sampling
        let ambiguity_ratio = if best_dist_sq > 1e-10 {
            second_dist_sq / best_dist_sq
        } else {
            f64::MAX
        };
        let voronoi_safe = ambiguity_ratio > 2.0;

        // CPR blend weight: full CPR at center, fade to oblique
        let cpr_blend = if !voronoi_safe || lateral.abs() > width_mm {
            0.0 // pure oblique
        } else if lateral.abs() < width_mm * 0.8 {
            1.0 // pure CPR
        } else {
            // Smooth transition zone (last 20% of width)
            let t = (width_mm - lateral.abs()) / (width_mm * 0.2);
            t.clamp(0.0, 1.0)
        };

        let final_val = if cpr_blend <= 0.0 {
            oblique_val
        } else {
            // CPR sample
            let sample_base = interp_pos + lateral * interp_normal;
            let mut cpr_val = f32::NEG_INFINITY;
            for &slab_off in &slab_offsets {
                let s = sample_base + slab_off * interp_binormal;
                let vz = (s[0] - origin[0]) * inv_spacing[0];
                let vy = (s[1] - origin[1]) * inv_spacing[1];
                let vx = (s[2] - origin[2]) * inv_spacing[2];
                let val = trilinear(volume, vz, vy, vx);
                if !val.is_nan() && val > cpr_val {
                    cpr_val = val;
                }
            }
            let cpr_val = if cpr_val == f32::NEG_INFINITY { f32::NAN } else { cpr_val };

            if cpr_blend >= 1.0 {
                cpr_val
            } else if cpr_val.is_nan() {
                oblique_val
            } else if oblique_val.is_nan() {
                cpr_val
            } else {
                // Blend
                cpr_val * cpr_blend as f32 + oblique_val * (1.0 - cpr_blend as f32)
            }
        };

        image[row * pixels_wide + col] = final_val;
    }
}
```

Key design decisions:
- **Oblique is always computed** — it's the "background" layer, artifact-free
- **CPR only kicks in** when: (a) within lateral width, (b) Voronoi-safe (no ambiguous segment assignment)
- **Smooth 20% blend zone** at the edge of the CPR region prevents visible seam
- **Voronoi check** (second_dist / first_dist > 2.0) catches the problem cases where the curve wraps back

- [ ] **Step 4: Run reference test and compare**

```bash
cd src-tauri
cargo test --lib -- --ignored --nocapture test_rca_reference
```

Then generate comparison:
```bash
cd .. && python3 -c "
import numpy as np; from PIL import Image; import pydicom
lo,hi=200-300,200+300
ds=pydicom.dcmread('path/to/syngo/rca.dcm')
ref=ds.pixel_array.astype(np.float32)*float(ds.RescaleSlope)+float(ds.RescaleIntercept)
ref_img=np.clip((ref-lo)/(hi-lo)*255,0,255).astype(np.uint8)
for rot in [0,90]:
    d=np.fromfile(f'test_output/rca_curved_rot{rot}.raw',np.float32).reshape(384,768)
    ours=np.nan_to_num(d,nan=lo)
    ours_img=np.clip((ours-lo)/(hi-lo)*255,0,255).astype(np.uint8)
    combined=Image.new('L',(1044,512),128)
    combined.paste(Image.fromarray(ref_img),(0,0))
    combined.paste(Image.fromarray(ours_img).resize((512,512),Image.LANCZOS),(532,0))
    combined.save(f'test_output/final_compare_rot{rot}.png')
"
```

**Verify BEFORE committing:**
- Rot0 and rot90 both show: vessel curve, surrounding chambers, no seam line, no blown-out regions
- NaN% should be < 5% (mostly volume boundaries)
- Compare side-by-side with syngo.via reference — shapes should be similar

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/pipeline/curved_cpr.rs
git commit -m "Two-layer curved CPR: oblique MIP context + CPR vessel with smooth blend"
```

## Task 2: Clean up unused code

**Files:**
- Modify: `src-tauri/src/pipeline/curved_cpr.rs`

- [ ] **Step 1: Remove the old `render_curved_cpr_pixeldriven` function**

The pixel-driven renderer is superseded. Remove it along with its `#[allow(dead_code)]` markers. Keep `compute_view_basis`, `compute_view_basis_pca`, `project_centerline_2d`, and `nearest_on_projected_centerline` (used by the projection info command and tests).

- [ ] **Step 2: Verify tests pass**

```bash
cargo test --lib
```

All 45 tests should pass.

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/pipeline/curved_cpr.rs
git commit -m "Remove superseded pixel-driven renderer"
```

## Verification

After both tasks:
1. `cargo test --lib` — 45 tests pass
2. `cargo check` — zero warnings
3. `npx svelte-check` — zero new errors
4. Run `test_rca_reference` — visually compare all 4 rotations against syngo.via
5. Run the app — load saved seeds, try curved CPR at different rotations, verify:
   - Full anatomical context visible (bones, chambers)
   - Vessel follows natural curve
   - No seam lines
   - No blown-out white regions
   - Cross-sections update when clicking on vessel
