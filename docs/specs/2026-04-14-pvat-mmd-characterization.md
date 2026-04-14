# PVAT Multi-Material Decomposition & Characterization Pipeline

**Date:** 2026-04-14
**Status:** Design
**Author:** Shu Nie + Claude

## Context

Current PCAT analysis uses FAI (Fat Attenuation Index) in Hounsfield Units to characterize pericoronary adipose tissue. FAI is protocol-dependent — the same tissue yields different HU values depending on kVp, kernel, scanner, and contrast timing (variance up to 33 HU). Multi-material decomposition (MMD) recovers intrinsic tissue composition (water/lipid/iodine/calcium volume fractions), which is protocol-independent.

This pipeline adds MMD-based PVAT characterization to the PCAT Workstation v2, starting with 66 patient UCI NAEOTOM CCTA datasets. The goal is to produce physiologically-informed radial-angular lipid composition profiles of pericoronary fat along the proximal RCA, which will later inform simulated phantom creation.

## Architecture: All-in-Workstation

Everything runs in the existing Tauri 2 + Svelte 5 + Rust app on Apple M3. No separate CLI needed because MMD runs only on a small ROI (~500K voxels vs 78M full volume), making it fast enough for interactive use.

### Cargo Workspace Layout

```
pcat-workstation-v2/
  Cargo.toml                    # workspace root
  crates/
    pcat-pipeline/              # shared domain logic library
      src/
        lib.rs
        dicom_loader.rs         # extended for dual-energy
        centerline.rs           # existing
        contour.rs              # existing
        cpr.rs                  # existing
        spline.rs               # existing
        interp.rs               # existing
        stats.rs                # existing
        voi.rs                  # existing
        mmd/                    # NEW
          mod.rs
          materials.rs          # LAC table for water/lipid/iodine/calcium
          direct.rs             # 3×3 matrix inversion (fallback/quick)
          pwsqs.rs              # Xue 2017 iterative solver
        active_contour.rs       # NEW: snake algorithm
        radial_angular.rs       # NEW: polar sampling + surface data
  src-tauri/                    # Tauri app (depends on pcat-pipeline)
    src/
      commands/
        dicom.rs                # extended for dual-energy
        pipeline.rs             # extended for MMD, snake, surface
      state.rs                  # extended with MmdVolumes, ContourAnnotations
  src/                          # Svelte frontend
    components/
      SnakeEditor.svelte        # NEW: active contour annotation UI
      MmdCrossSection.svelte    # NEW: cross-section with MMD overlay
      SurfacePlotPanel.svelte   # NEW: Plotly.js 3D surface plots
      CrossSectionGrid.svelte   # NEW: grid of 20 cross-sections
```

## Workflow (Per Patient)

```
1. Load dual-energy DICOM (70 + 150 keV MonoPlus VMI+)
2. Place waypoints → extract centerline along proximal 40mm of main RCA
3. Generate 20 cross-sections at 2mm intervals
4. On each cross-section (HU image):
   a. Auto-detect vessel wall (existing polar threshold + gradient)
   b. Auto-compute 1-vessel-diameter circular boundary
   c. Semi-auto annotate outer fat contour (active contour / snake)
5. Build 3D ROI mask from all 20 contour annotations
6. Run PWSQS MMD on ROI voxels only (~500K voxels, seconds on M3)
7. Sample MMD results: 16 angular bins × radial (0→20mm) per cross-section
8. Generate surface plots: θ × r × lipid_fraction and θ × r × density
```

Key decision: **annotate on HU first** (clean images, fat visible at -190 to -30 HU), **then run MMD** on the annotated ROI. Optional refinement on the MMD lipid map if boundaries look different.

## Phase 0: Cargo Workspace Restructuring

Move `src-tauri/src/pipeline/` into `crates/pcat-pipeline/`. The Tauri app re-imports from the shared crate. All existing tests continue to pass.

**Files to move:**
- `src-tauri/src/pipeline/*.rs` → `crates/pcat-pipeline/src/*.rs`

**Files to update:**
- `src-tauri/Cargo.toml` — add dependency on `pcat-pipeline`
- `src-tauri/src/commands/*.rs` — update `use` paths

## Phase 1: Dual-Energy DICOM Loading

### Series Identification

Each patient folder contains multiple DICOM series. The 70 keV and 150 keV MonoPlus VMI+ series are identified by:
- `SeriesDescription` (0008,103E) — contains keV label, but **may be mislabeled** (see memory: MonoPlus keV mislabeling)
- `ImageComments` — has true keV for identified data (stripped by de-identification)
- Pixel comparison — for de-identified data, compare series pixel data to identify truly unique reconstructions

### Implementation

New function: `scan_dicom_series(dir) -> Vec<SeriesInfo>` — walks directory, groups by SeriesInstanceUID, extracts metadata.

New struct:
```rust
pub struct DualEnergyVolume {
    pub low: Arc<Array3<f32>>,     // 70 keV HU
    pub high: Arc<Array3<f32>>,    // 150 keV HU
    pub low_energy_kev: f64,       // 70.0
    pub high_energy_kev: f64,      // 150.0
    pub spacing: [f64; 3],
    pub origin: [f64; 3],
    pub direction: [f64; 9],
}
```

Frontend: series selection UI showing all detected series with keV labels. User explicitly selects which series is low-energy and which is high-energy.

## Phase 2: Cross-Section + Active Contour Annotation

### Cross-Section Generation

20 cross-sections at 2mm intervals along proximal 40mm of main RCA. Uses existing `CprFrame` + Bishop frame + trilinear interpolation. Display as HU images with FAI colormap overlay.

### Active Contour Algorithm

**Energy functional:**
```
E = α·E_elasticity + β·E_bending + γ·E_image + δ·E_balloon
```

- `E_elasticity = Σ|v_i - v_{i-1}|²` — keeps contour smooth
- `E_bending = Σ|v_{i-1} - 2v_i + v_{i+1}|²` — penalizes sharp bends
- `E_image = -|∇I(v_i)|²` — attracts to HU gradient edges (fat boundary)
- `E_balloon` — pressure force to expand/contract (expand in fat, contract in non-fat)

**Implementation:** `crates/pcat-pipeline/src/active_contour.rs`

```rust
pub struct SnakeParams {
    pub alpha: f64,       // elasticity weight
    pub beta: f64,        // bending weight
    pub gamma: f64,       // image force weight
    pub balloon: f64,     // balloon pressure
    pub step_size: f64,   // time step
    pub n_points: usize,  // contour points (default 72)
}

pub fn evolve_snake(
    contour: &mut Vec<[f64; 2]>,
    gradient_field: &Array2<[f64; 2]>,  // precomputed GVF or gradient
    params: &SnakeParams,
    n_iterations: usize,
) -> ConvergenceInfo;
```

**Interactive workflow:**
1. Auto-initialize: circular contour at 1× vessel diameter from center
2. User clicks "Evolve" → ~200-300 iterations → contour converges
3. User drags control points where it didn't fit (添加 waypoint and drag)
4. User clicks "Evolve" again → re-converges around corrections
5. Repeat until satisfied → "Accept" → next cross-section

**Two auto-computed boundaries per cross-section:**
- Inner: vessel wall (existing `contour.rs`)
- 1D circle: vessel center + vessel_diameter radius (auto)

### ROI Mask Construction

Interpolate the 20 outer contours along the centerline to create a continuous 3D mask. For each voxel in the volume, determine if it falls within any cross-section's contour boundary. Uses the existing Bishop frame coordinate system.

## Phase 3: ROI-Only Multi-Material Decomposition

### Material Library (4 materials)

| Material | μ(70 keV) cm⁻¹ | μ(150 keV) cm⁻¹ | ρ (g/cm³) |
|----------|----------------|-----------------|-----------|
| Water | ~0.193 | ~0.149 | 1.000 |
| Lipid (adipose) | ~0.172 | ~0.142 | 0.950 |
| Iodine | ~1.94 | ~0.547 | 4.930 |
| Calcium (hydroxyapatite) | ~0.573 | ~0.300 | 3.180 |

Values from NIST XCOM tables. Exact values will be computed using effective energies (keV × ~0.4 for effective energy, or spectrum-weighted).

### Xue 2017 PWSQS Algorithm

**Material sparsity constraint:** Each voxel composed of at most 3 of the 4 materials. Enumerate all C(4,3) = 4 material triplets. For each voxel, select the triplet with minimum Euclidean distance to the LAC pair.

**Objective function:**
```
min_f  Φ(f) = L(f) + β·R(f)
where:
  L(f) = (Af - μ)ᵀ V⁻¹ (Af - μ)          # data fidelity (weighted least squares)
  R(f) = Σ_l β_l · Σ_p Σ_{q∈N(p)} ψ(f_lp - f_lq)  # edge-preserving regularization
  ψ(t) = (δ²/3)·(√(1 + 3t²/δ²) - 1)      # Huber-like penalty
```

**PWSQS iteration:** Pixel-wise separable quadratic surrogate. Each iteration:
1. Compute Hessian H and gradient q for each voxel
2. For each material triplet τ = (i,j,k): solve 3×3 QP
3. Select optimal triplet per voxel (minimum objective)
4. Update volume fractions
5. Check convergence (||f_new - f_old|| < tolerance)

**Rust implementation:**
- `crates/pcat-pipeline/src/mmd/pwsqs.rs`
- Uses `rayon` for parallel voxel updates within each iteration
- Progress callback for UI (iteration count, convergence metric)
- ROI mask: skip voxels outside mask

**Fallback:** `crates/pcat-pipeline/src/mmd/direct.rs` — simple 3×3 matrix inversion per voxel (3 materials only: water/lipid/iodine). For quick validation before PWSQS is stable.

### Output

```rust
pub struct MmdResult {
    pub water: Array3<f32>,     // volume fraction [0, 1]
    pub lipid: Array3<f32>,     // volume fraction [0, 1]
    pub iodine: Array3<f32>,    // volume fraction [0, 1]
    pub calcium: Array3<f32>,   // volume fraction [0, 1]
    pub density: Array3<f32>,   // mass density (mg/mL)
    pub mask: Array3<bool>,     // which voxels were decomposed
    pub iterations: usize,
    pub converged: bool,
}
```

Mass density per voxel: `ρ(x) = f_w·ρ_water + f_l·ρ_lipid + f_i·ρ_iodine + f_c·ρ_calcium`

## Phase 4: Radial-Angular Sampling + Surface Plots

### Sampling Grid

Per cross-section, within the annotated fat region:
- **16 angular bins** at 22.5° intervals (0°, 22.5°, ..., 337.5°), full 360°
- **Radial sampling** from vessel wall (r=0) outward to min(20mm, outer contour), at 0.5mm steps
- At each (θ, r): look up two values from MMD volumes via trilinear interpolation

### Two Z-Values

1. **Z1: Pure lipid volumetric fraction** — from `mmd_result.lipid` (dimensionless, 0 to 1)
2. **Z2: Mass density** — from `mmd_result.density` (mg/mL)

### Data Structure

```rust
pub struct CrossSectionSurface {
    pub arc_mm: f64,                    // position along centerline
    pub theta_deg: Vec<f64>,            // 16 angular bin centers
    pub r_mm: Vec<f64>,                 // radial positions
    pub lipid_surface: Array2<f32>,     // [16, n_radial]
    pub density_surface: Array2<f32>,   // [16, n_radial]
    pub max_r_per_theta: Vec<f64>,      // contour boundary at each angle
}
```

### Surface Plot Visualization

Plotly.js `surface` trace (already in project dependencies):
```javascript
Plotly.newPlot(div, [{
  type: 'surface',
  x: theta_grid,        // 16 bins: 0-360°
  y: r_grid,            // 0-20mm
  z: lipid_surface,     // [16 × n_radial]
  colorscale: 'Viridis',
}], {
  scene: {
    xaxis: { title: 'θ (degrees)' },
    yaxis: { title: 'r (mm from wall)' },
    zaxis: { title: 'Lipid fraction' },
  }
});
```

**4th dimension (arc-length):** Slider control to navigate through 20 cross-sections. Each position shows its (θ, r, z) surface. Animation option to sweep along the vessel.

**Total output:** 20 cross-sections × 2 z-values = 40 surface plots per patient.

## Phase 5: Batch Processing

After validating on 1 reference patient:
1. "Process All" button iterates through remaining 65 patients
2. Each patient requires: waypoint placement (manual), contour annotation (semi-auto, ~20 cross-sections), MMD (auto), surface extraction (auto)
3. Per-patient data saved as JSON: seeds, contour annotations, MMD parameters, surface data
4. Export: CSV of all surface data for downstream statistical analysis

### Data Access

- Source: `smb://160.87.12.113/Molloilab/Shu Nie/UCI NAEOTOM CCTA Data`
- Read-only DICOM loading via SMB mount is acceptable (read-once)
- Never write through SMB mount (use scp for output files)
- Results saved locally per patient

## Verification Plan

### Phase 0
- All existing tests pass after workspace restructuring
- `cargo test` in workspace root

### Phase 1
- Load a test patient's dual-energy DICOM
- Verify both volumes have matching geometry (dimensions, spacing, origin)
- Verify keV identification handles mislabeled SeriesDescription

### Phase 2
- Visual inspection: snake converges to fat boundary on HU cross-sections
- Drag control points, re-evolve — contour adjusts
- Compare auto-detected vessel wall with manual measurement

### Phase 3
- Synthetic phantom test: create known water/lipid/iodine/calcium mixture, verify decomposition recovers ground truth within tolerance
- Lumen sanity check: verify high iodine + water in contrast-enhanced lumen
- Compare direct inversion vs PWSQS on same patient: PWSQS should be smoother with similar mean values in homogeneous regions

### Phase 4
- Surface plot for one cross-section: verify θ and r axes are correct
- Compare lipid fraction surface shape with HU radial profile (existing FAI analysis)
- Check that angular bins outside the fat contour show ~0 lipid

### End-to-End
- Full pipeline on 1 reference patient: DICOM → waypoints → cross-sections → annotation → MMD → surface plots
- Inspect surface plots: does the lipid distribution match the hypothesis? (radial symmetry in circular zone, angular asymmetry in triangular extension)
