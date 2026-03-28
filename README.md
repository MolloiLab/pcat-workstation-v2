# PCAT Workstation v2

A clinical workstation for **Pericoronary Adipose Tissue (PCAT)** analysis — Fat Attenuation Index (FAI) measurement around coronary arteries from cardiac CT.

Built with **Tauri v2** (Rust backend) + **Svelte 5** frontend + **cornerstone3D** for medical image viewing.

## Features

### DICOM Viewer
- Load DICOM CT volumes from folder
- 3-pane MPR (Axial, Coronal, Sagittal) with synchronized crosshairs
- Window/Level adjustment (right-drag), scroll through slices
- Pinch-to-zoom on trackpad
- Recent DICOM history

### Seed Placement & Centerline
- Click-to-place seeds on MPR views for RCA, LAD, LCx
- Per-vessel color coding (RCA green, LAD orange, LCx blue)
- Drag-to-move seeds, Delete/Backspace to remove
- Cubic spline centerline with real-time update
- Ostium marker (Shift+click on CPR)
- Undo/Redo (Cmd+Z / Cmd+Shift+Z)
- Per-patient seed save/load (auto-loads on volume change)

### CPR Views
- **Straightened CPR**: vessel unrolled into columns, cross-sections at 3 needle positions (A/B/C)
- **Curved CPR**: vessel follows natural projected path (PCA-based viewing with rotation)
  - Depth-centered average intensity — perivascular fat visible alongside vessel
  - Isotropic pixels — no aspect-ratio distortion
  - Works at all rotation angles
- **FAI overlay**: toggle button colors fat-range pixels green (healthy) to red (inflamed)
- Vessel diameter measurement on cross-sections
- Rotation slider, needle offset control

### FAI Analysis Pipeline
- **6-stage pipeline**: centerline densification, arc-length clipping (0-40mm), radius estimation, contour extraction (360 angles), VOI construction (CRISP-CT: 1mm gap + 3mm ring), FAI statistics
- **Overview**: per-vessel risk cards (HIGH/LOW based on -70.1 HU threshold)
- **HU Histogram**: distribution within FAI window (-190 to -30 HU)
- **Radial Profile**: mean HU vs distance from vessel wall (1-20mm)
- **Angular Asymmetry**: 8-sector ring visualization with per-sector mean HU
- Pipeline results saved/loaded with seeds

## Tech Stack

| Layer | Technology |
|-------|-----------|
| Backend | Rust (Tauri v2) — DICOM loading, volume processing, CPR rendering, FAI computation |
| Frontend | Svelte 5, TypeScript, Tailwind CSS v4 |
| Medical Imaging | cornerstone3D (MPR viewing) |
| Charts | Plotly.js (histograms, radial profiles) |
| IPC | Tauri commands with raw binary responses |

## Development

### Prerequisites

- [Rust](https://rustup.rs/) (stable)
- [Node.js](https://nodejs.org/) >= 18
- [Tauri CLI](https://tauri.app/start/): `cargo install tauri-cli`

### Setup

```bash
git clone <repo-url>
cd pcat-workstation-v2
npm install
```

### Run (dev)

```bash
cargo tauri dev
```

### Build (release)

```bash
cargo tauri build
```

The `.dmg` / `.app` output is in `src-tauri/target/release/bundle/`.

## Project Structure

```
src/                          # Svelte frontend
  components/
    MprPanel.svelte           # 2x2 MPR grid
    SliceViewport.svelte      # Single cornerstone viewport
    CprView.svelte            # CPR + cross-sections + FAI overlay
    AnalysisDashboard.svelte  # FAI results (4 tabs)
    SeedOverlay.svelte        # SVG seed/centerline overlay
    CrossSection.svelte       # Single cross-section panel
    ContextPanel.svelte       # Bottom-right panel (CPR | Analysis tabs)
  lib/
    stores/                   # Svelte 5 reactive stores
      seedStore.svelte.ts     # Per-vessel seeds, undo/redo
      volumeStore.svelte.ts   # Loaded volume metadata
      pipelineStore.svelte.ts # Pipeline state + results
    api.ts                    # Tauri IPC wrappers
    cprProjection.ts          # 3D <-> 2D CPR coordinate mapping
    navigation.ts             # Cross-view MPR navigation

src-tauri/                    # Rust backend
  src/
    commands/
      dicom.rs                # DICOM loading, seed save/load
      cpr.rs                  # CPR frame + rendering commands
      pipeline.rs             # FAI analysis pipeline orchestration
    pipeline/
      cpr.rs                  # Straightened CPR, cross-sections, Bishop frame
      curved_cpr.rs           # Curved CPR renderer (PCA + depth-centered AIP)
      spline.rs               # Cubic spline interpolation
      contour.rs              # Vessel boundary extraction (polar radial)
      voi.rs                  # Perivascular VOI mask (CRISP-CT)
      stats.rs                # FAI stats, radial profile, angular asymmetry
      centerline.rs           # Centerline clipping, radius estimation
      dicom_loader.rs         # DICOM directory parsing
      interp.rs               # Trilinear interpolation
    state.rs                  # AppState (volume, results, CPR frame cache)
```

## References

- Oikonomou EK et al. "Non-invasive detection of coronary inflammation using computed tomography and prediction of residual cardiovascular risk." *Eur Heart J*. 2018.
- CRISP-CT: Coronary Inflammation and Structural Plaque characteristics by CT.
- Antonopoulos AS et al. "Detecting human coronary inflammation by imaging perivascular fat." *Sci Transl Med*. 2017.

## License

Research use only. Not for clinical diagnosis.
