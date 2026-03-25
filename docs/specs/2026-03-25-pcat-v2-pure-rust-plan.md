# PCAT Workstation v2 — Pure Rust Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a Tauri v2 + Svelte 5 desktop app for PCAT/FAI analysis with ALL compute in Rust — no Python sidecar.

**Architecture:** 田字格 (2×2) layout: 3 MPR viewports (cornerstone3D) + 1 context panel (CPR or FAI dashboard). Rust backend handles DICOM loading (dicom-rs), volume management (ndarray), CPR generation, contour extraction, VOI construction, and FAI statistics. Frontend handles seed placement + spline computation (TypeScript). Communication via Tauri IPC commands.

**Tech Stack:** Tauri v2, Svelte 5, TypeScript, cornerstone3D v4, Plotly.js, Tailwind CSS v4, Rust (dicom-rs, ndarray, nalgebra, tauri-plugin-dialog)

**Spec:** `docs/specs/2026-03-25-pcat-v2-revised-design.md`

---

## What Changes From Python Sidecar Plan

| Component | Before (Python) | After (Rust) |
|---|---|---|
| DICOM loading | pydicom via FastAPI | dicom-rs in Rust |
| Volume storage | numpy array in Python process | ndarray in Rust AppState |
| Slice serving | HTTP GET /volume/{id}/slice | Tauri command `get_slice` |
| CPR computation | Python `_compute_cpr_data()` via HTTP | Rust `compute_cpr` command |
| Contour extraction | Python `extract_vessel_contours()` | Rust `extract_contours` command |
| VOI + FAI | Python functions via HTTP | Rust commands |
| Sidecar lifecycle | Rust spawns/manages Python process | **Eliminated** |
| Distribution | Tauri binary + PyInstaller (~100MB) | **Single binary (~15MB)** |

**Frontend stays identical** — Svelte, cornerstone3D, stores, spline.ts. Only the `api.ts` changes from HTTP fetch to Tauri `invoke()`.

---

## File Structure

### Rust Backend (`src-tauri/src/`)

```
src-tauri/src/
├── main.rs                    # Binary entry (exists)
├── lib.rs                     # Tauri setup + command registration (rewrite)
├── state.rs                   # AppState with volume storage (rewrite)
├── commands/
│   ├── mod.rs                 # Re-export all command modules
│   ├── dicom.rs               # open_dicom_dialog, load_dicom, scan_dicom
│   ├── volume.rs              # get_slice, get_volume_info
│   ├── cpr.rs                 # compute_cpr, compute_cross_section
│   ├── pipeline.rs            # run_pipeline (contour + VOI + FAI)
│   └── session.rs             # save_session, load_session
├── pipeline/
│   ├── mod.rs                 # Re-export pipeline modules
│   ├── dicom_loader.rs        # DICOM directory → ndarray volume
│   ├── centerline.rs          # clip_by_arclength, estimate_radii
│   ├── contour.rs             # extract_vessel_contours, ContourResult
│   ├── voi.rs                 # build_contour_based_voi
│   ├── stats.rs               # compute_pcat_stats, FaiResults
│   ├── cpr.rs                 # compute_cpr_data, Bishop frame, interpolation
│   └── interp.rs              # trilinear/tricubic volume interpolation
└── error.rs                   # AppError type
```

### Frontend (`src/`) — Mostly Unchanged

```
src/
├── App.svelte                 # Rewrite for 田字格
├── app.css                    # Exists (Horos dark theme)
├── lib/
│   ├── api.ts                 # REWRITE: Tauri invoke() instead of HTTP fetch
│   ├── spline.ts              # NEW: cubic spline (TypeScript)
│   ├── cornerstone/           # Exists from Tasks 1.2-1.3 (keep as-is)
│   │   ├── init.ts
│   │   ├── tools.ts
│   │   └── volumeLoader.ts    # REWRITE: fetch slices via Tauri command
│   └── stores/                # Exists (keep as-is, add new stores)
│       ├── volumeStore.svelte.ts
│       ├── viewportStore.svelte.ts
│       ├── seedStore.svelte.ts    # NEW
│       └── pipelineStore.svelte.ts # NEW
├── components/
│   ├── MprPanel.svelte        # NEW: 田字格 container
│   ├── SliceViewport.svelte   # EXISTS (keep)
│   ├── ContextPanel.svelte    # EXISTS (keep)
│   ├── CprView.svelte         # NEW
│   ├── CrossSection.svelte    # NEW
│   ├── SeedToolbar.svelte     # NEW
│   ├── AnalysisDashboard.svelte # NEW
│   └── ProgressOverlay.svelte # NEW
```

### Delete

```
pipeline_server/               # ENTIRE DIRECTORY — no longer needed
src-tauri/src/sidecar.rs       # Sidecar manager — no longer needed
```

---

## Chunk 1: Phase 1 — Rust DICOM + MPR

### Task 1.1: Rust Project Restructure + Dependencies

**Files:**
- Modify: `src-tauri/Cargo.toml`
- Delete: `src-tauri/src/sidecar.rs`
- Create: `src-tauri/src/error.rs`
- Create: `src-tauri/src/commands/mod.rs`
- Create: `src-tauri/src/pipeline/mod.rs`
- Rewrite: `src-tauri/src/state.rs`
- Rewrite: `src-tauri/src/lib.rs`

- [ ] **Step 1: Update `Cargo.toml` with new dependencies**

Remove `reqwest` (no more HTTP calls). Add:
```toml
[dependencies]
tauri = { version = "2", features = [] }
tauri-plugin-dialog = "2"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["full"] }
thiserror = "2"
ndarray = { version = "0.16", features = ["serde"] }
nalgebra = "0.33"
dicom = "0.9"
dicom-pixeldata = { version = "0.9", features = ["ndarray"] }
bytemuck = { version = "1", features = ["derive"] }
walkdir = "2"
```

Remove `tauri-plugin-shell` (no sidecar), remove `reqwest`.

- [ ] **Step 2: Delete `sidecar.rs`**

- [ ] **Step 3: Create `src-tauri/src/error.rs`**

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("DICOM error: {0}")]
    Dicom(String),
    #[error("No volume loaded")]
    NoVolume,
    #[error("Invalid argument: {0}")]
    InvalidArg(String),
    #[error("Pipeline error: {0}")]
    Pipeline(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

impl serde::Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where S: serde::Serializer {
        serializer.serialize_str(&self.to_string())
    }
}
```

- [ ] **Step 4: Rewrite `state.rs`**

Replace sidecar-based state with volume-holding state:

```rust
use ndarray::Array3;
use std::collections::HashMap;
use serde::{Serialize, Deserialize};

pub struct LoadedVolume {
    pub data: Array3<f32>,          // (Z, Y, X) HU values
    pub spacing: [f64; 3],          // [sz, sy, sx] mm
    pub origin: [f64; 3],           // [oz, oy, ox] mm
    pub direction: [f64; 9],        // row-major 3x3
    pub window_center: f64,
    pub window_width: f64,
    pub patient_name: String,
    pub study_description: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Vessel { LAD, LCx, RCA }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisResults {
    pub fai_values: HashMap<Vessel, f64>,
    pub fat_fractions: HashMap<Vessel, f64>,
    pub fai_risks: HashMap<Vessel, String>,
}

pub struct AppState {
    pub volume: Option<LoadedVolume>,
    pub analysis_results: Option<AnalysisResults>,
}

impl AppState {
    pub fn new() -> Self {
        Self { volume: None, analysis_results: None }
    }
}
```

- [ ] **Step 5: Create empty module files**

Create `src-tauri/src/commands/mod.rs` and `src-tauri/src/pipeline/mod.rs` with placeholder module declarations.

- [ ] **Step 6: Rewrite `lib.rs`**

Minimal Tauri setup without sidecar. Register `tauri-plugin-dialog`. Empty command handler for now.

```rust
mod commands;
mod error;
mod pipeline;
mod state;

use state::AppState;
use std::sync::Mutex;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            app.manage(Mutex::new(AppState::new()));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

- [ ] **Step 7: Verify it compiles**

```bash
cd src-tauri && cargo check
```

- [ ] **Step 8: Commit**

```bash
git add src-tauri/ && git rm src-tauri/src/sidecar.rs
git commit -m "refactor: remove Python sidecar, restructure for pure Rust backend"
```

---

### Task 1.2: DICOM Loader in Rust

**Files:**
- Create: `src-tauri/src/pipeline/dicom_loader.rs`
- Modify: `src-tauri/src/pipeline/mod.rs`

- [ ] **Step 1: Implement `dicom_loader.rs`**

```rust
// src-tauri/src/pipeline/dicom_loader.rs
use crate::error::AppError;
use crate::state::LoadedVolume;
use dicom::object::open_file;
use dicom::dictionary_std::tags;
use ndarray::Array3;
use std::path::Path;
use walkdir::WalkDir;

pub fn load_dicom_directory(dir: &Path) -> Result<LoadedVolume, AppError> {
    // 1. Scan directory for .dcm files
    // 2. Read each file, extract: pixel data, ImagePositionPatient, spacing, etc.
    // 3. Sort slices by Z position (ImagePositionPatient[2])
    // 4. Stack into Array3<f32> with rescale slope/intercept → HU
    // 5. Return LoadedVolume
}
```

Core algorithm:
1. Walk directory with `walkdir`, collect all files
2. For each file: `open_file()`, read `PixelData`, `ImagePositionPatient`, `PixelSpacing`, `SliceThickness`, `RescaleSlope`, `RescaleIntercept`, `WindowCenter`, `WindowWidth`, `PatientName`, `StudyDescription`
3. Sort by Z position
4. Use `dicom-pixeldata` to decode pixel data as `i16`, apply `HU = pixel * slope + intercept`
5. Stack into `Array3<f32>` shape (n_slices, rows, cols)
6. Clamp: values ≤ -8192 → -1024 (air), values > 3095 → 3095

- [ ] **Step 2: Write Rust test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    // Test with a small synthetic DICOM or skip if no test data
    #[test]
    fn test_load_returns_correct_shape() {
        // If test DICOM exists:
        // let vol = load_dicom_directory(Path::new("test_data/dicom")).unwrap();
        // assert_eq!(vol.data.shape(), &[n_slices, rows, cols]);
    }
}
```

- [ ] **Step 3: Commit**

```bash
git commit -m "feat(rust): DICOM loader using dicom-rs + ndarray"
```

---

### Task 1.3: Volume Commands (Slice Serving)

**Files:**
- Create: `src-tauri/src/commands/dicom.rs`
- Create: `src-tauri/src/commands/volume.rs`
- Modify: `src-tauri/src/commands/mod.rs`
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Create `commands/dicom.rs`**

```rust
use crate::error::AppError;
use crate::pipeline::dicom_loader;
use crate::state::AppState;
use std::sync::Mutex;
use std::path::PathBuf;

#[tauri::command]
pub async fn open_dicom_dialog(app: tauri::AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let path = app.dialog().file().blocking_pick_folder();
    Ok(path.map(|p| p.to_string_lossy().to_string()))
}

#[derive(serde::Serialize)]
pub struct VolumeInfo {
    pub shape: [usize; 3],       // [Z, Y, X]
    pub spacing: [f64; 3],       // [sz, sy, sx]
    pub origin: [f64; 3],
    pub window_center: f64,
    pub window_width: f64,
    pub patient_name: String,
    pub study_description: String,
}

#[tauri::command]
pub async fn load_dicom(
    path: String,
    state: tauri::State<'_, Mutex<AppState>>,
) -> Result<VolumeInfo, String> {
    let vol = dicom_loader::load_dicom_directory(&PathBuf::from(&path))
        .map_err(|e| e.to_string())?;

    let shape = vol.data.raw_dim();
    let info = VolumeInfo {
        shape: [shape[0], shape[1], shape[2]],
        spacing: vol.spacing,
        origin: vol.origin,
        window_center: vol.window_center,
        window_width: vol.window_width,
        patient_name: vol.patient_name.clone(),
        study_description: vol.study_description.clone(),
    };

    let mut app_state = state.lock().map_err(|e| e.to_string())?;
    app_state.volume = Some(vol);

    Ok(info)
}
```

- [ ] **Step 2: Create `commands/volume.rs`**

```rust
use crate::state::AppState;
use std::sync::Mutex;

/// Return a single axial/coronal/sagittal slice as raw int16 bytes.
/// Frontend cornerstone3D volume loader calls this per-slice.
#[tauri::command]
pub async fn get_slice(
    axis: String,
    idx: usize,
    state: tauri::State<'_, Mutex<AppState>>,
) -> Result<Vec<u8>, String> {
    let app_state = state.lock().map_err(|e| e.to_string())?;
    let vol = app_state.volume.as_ref().ok_or("No volume loaded")?;
    let data = &vol.data;
    let shape = data.raw_dim();

    let slice_data: Vec<i16> = match axis.as_str() {
        "axial" => {
            let z = idx.min(shape[0] - 1);
            data.slice(ndarray::s![z, .., ..]).iter().map(|&v| v as i16).collect()
        }
        "coronal" => {
            let y = idx.min(shape[1] - 1);
            data.slice(ndarray::s![.., y, ..]).iter().map(|&v| v as i16).collect()
        }
        "sagittal" => {
            let x = idx.min(shape[2] - 1);
            data.slice(ndarray::s![.., .., x]).iter().map(|&v| v as i16).collect()
        }
        _ => return Err("Invalid axis".into()),
    };

    // Convert to raw bytes
    Ok(bytemuck::cast_slice(&slice_data).to_vec())
}
```

- [ ] **Step 3: Register commands in `lib.rs`**

```rust
.invoke_handler(tauri::generate_handler![
    commands::dicom::open_dicom_dialog,
    commands::dicom::load_dicom,
    commands::volume::get_slice,
])
```

- [ ] **Step 4: Verify compilation**

```bash
cargo check
```

- [ ] **Step 5: Commit**

```bash
git commit -m "feat(rust): DICOM + volume slice commands via Tauri IPC"
```

---

### Task 1.4: Frontend — Rewrite API Layer + Volume Loader

**Files:**
- Rewrite: `src/lib/api.ts` — Tauri invoke instead of HTTP
- Rewrite: `src/lib/cornerstone/volumeLoader.ts` — fetch slices via Tauri command

- [ ] **Step 1: Rewrite `api.ts`**

Replace all HTTP fetch calls with Tauri `invoke()`:

```typescript
// src/lib/api.ts
import { invoke } from '@tauri-apps/api/core';

export type VolumeInfo = {
  shape: [number, number, number];
  spacing: [number, number, number];
  origin: [number, number, number];
  window_center: number;
  window_width: number;
  patient_name: string;
  study_description: string;
};

export async function openDicomDialog(): Promise<string | null> {
  return invoke<string | null>('open_dicom_dialog');
}

export async function loadDicom(path: string): Promise<VolumeInfo> {
  return invoke<VolumeInfo>('load_dicom', { path });
}

export async function getSlice(axis: string, idx: number): Promise<ArrayBuffer> {
  const bytes = await invoke<number[]>('get_slice', { axis, idx });
  return new Uint8Array(bytes).buffer;
}
```

Note: Tauri IPC serializes `Vec<u8>` as a JSON array of numbers. For large slices this is inefficient. Alternative: use Tauri's raw IPC or base64 encoding. The implementer should benchmark and optimize if needed — possible approaches:
- Return base64 string and decode in frontend
- Use `tauri::ipc::Response` with raw bytes (Tauri v2 feature)

- [ ] **Step 2: Rewrite `volumeLoader.ts`**

Change from `sidecarFetch()` to `getSlice()` Tauri command:

```typescript
import { getSlice } from '$lib/api';
// ... rest stays similar, just replace the fetch call:
const arrayBuf = await getSlice('axial', z);
const sliceData = new Int16Array(arrayBuf);
```

- [ ] **Step 3: Commit**

```bash
git commit -m "feat(frontend): rewrite API layer for Tauri IPC (no HTTP)"
```

---

### Task 1.5: Frontend — 田字格 MPR Layout

**Files:**
- Create: `src/components/MprPanel.svelte`
- Modify: `src/components/SliceViewport.svelte`
- Modify: `src/components/ContextPanel.svelte`
- Rewrite: `src/App.svelte`

- [ ] **Step 1: Create `MprPanel.svelte`**

2×2 CSS Grid. Three `SliceViewport` instances + one `ContextPanel`. Initializes cornerstone3D on mount, sets up viewports and tool group.

- [ ] **Step 2: Update `SliceViewport.svelte`**

Ensure it provides its container div to parent for cornerstone3D viewport setup. Accept `orientation` and `viewportId` props.

- [ ] **Step 3: Rewrite `App.svelte`**

- Remove sidebar
- Keep toolbar (PCAT Workstation title, Open DICOM button, Settings button)
- Main area = `<MprPanel />`
- Keep status bar
- Wire "Open DICOM": `openDicomDialog()` → `loadDicom(path)` → store in volumeStore → `loadVolumeFromSidecar()` (renamed to `loadVolume()`) → set cornerstone volume ID
- Show loading progress bar during slice fetching

- [ ] **Step 4: Test**

```
cargo tauri dev → Open DICOM → 3 MPR views render → scroll + W/L work
```

- [ ] **Step 5: Commit**

```bash
git commit -m "feat: Phase 1 complete — 田字格 MPR layout with Rust DICOM loading"
```

---

## Chunk 2: Phase 2 — Seeds + Live Centerline + CPR

### Task 2.1: Seed Store + TypeScript Spline

**Files:**
- Create: `src/lib/stores/seedStore.svelte.ts`
- Create: `src/lib/spline.ts`

Identical to the Python-sidecar plan — these are frontend-only. The seed store holds per-vessel seeds, recomputes cubic spline centerline instantly on any change. See previous plan for full code.

- [ ] **Step 1: Create `seedStore.svelte.ts`** with Svelte 5 runes
- [ ] **Step 2: Create `spline.ts`** — natural cubic spline, ~150 LOC
- [ ] **Step 3: Commit**

---

### Task 2.2: Seed Placement on MPR Views

**Files:**
- Create: `src/components/SeedToolbar.svelte`
- Modify: `src/components/SliceViewport.svelte` — click handler
- Modify: `src/App.svelte` — add SeedToolbar

Identical to previous plan — purely frontend work.

- [ ] **Step 1: Create `SeedToolbar.svelte`** — vessel selector buttons
- [ ] **Step 2: Add click handler** to SliceViewport — canvas click → world coords → seedStore.addSeed()
- [ ] **Step 3: Render seed markers** as overlays on viewports
- [ ] **Step 4: Render centerline** as polyline overlay
- [ ] **Step 5: Commit**

---

### Task 2.3: CPR Computation in Rust

**Files:**
- Create: `src-tauri/src/pipeline/interp.rs` — volume interpolation
- Create: `src-tauri/src/pipeline/cpr.rs` — CPR generation + Bishop frame
- Create: `src-tauri/src/commands/cpr.rs`
- Modify: `src-tauri/src/commands/mod.rs`
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Create `pipeline/interp.rs`**

Trilinear interpolation for sampling a 3D volume at arbitrary float coordinates:

```rust
use ndarray::Array3;

/// Trilinear interpolation on a (Z, Y, X) float32 volume.
/// Returns f32::NAN if out of bounds.
pub fn trilinear(vol: &Array3<f32>, z: f64, y: f64, x: f64) -> f32 {
    let shape = vol.raw_dim();
    if z < 0.0 || y < 0.0 || x < 0.0
       || z >= (shape[0] - 1) as f64
       || y >= (shape[1] - 1) as f64
       || x >= (shape[2] - 1) as f64 {
        return f32::NAN;
    }
    let z0 = z.floor() as usize; let z1 = z0 + 1;
    let y0 = y.floor() as usize; let y1 = y0 + 1;
    let x0 = x.floor() as usize; let x1 = x0 + 1;
    let zf = (z - z0 as f64) as f32;
    let yf = (y - y0 as f64) as f32;
    let xf = (x - x0 as f64) as f32;

    let c000 = vol[[z0,y0,x0]]; let c001 = vol[[z0,y0,x1]];
    let c010 = vol[[z0,y1,x0]]; let c011 = vol[[z0,y1,x1]];
    let c100 = vol[[z1,y0,x0]]; let c101 = vol[[z1,y0,x1]];
    let c110 = vol[[z1,y1,x0]]; let c111 = vol[[z1,y1,x1]];

    let c00 = c000 * (1.0 - xf) + c001 * xf;
    let c01 = c010 * (1.0 - xf) + c011 * xf;
    let c10 = c100 * (1.0 - xf) + c101 * xf;
    let c11 = c110 * (1.0 - xf) + c111 * xf;
    let c0 = c00 * (1.0 - yf) + c01 * yf;
    let c1 = c10 * (1.0 - yf) + c11 * yf;
    c0 * (1.0 - zf) + c1 * zf
}
```

- [ ] **Step 2: Create `pipeline/cpr.rs`**

```rust
use nalgebra::Vector3;
use ndarray::Array3;
use crate::pipeline::interp::trilinear;

pub struct CprResult {
    pub image: Vec<f32>,         // pixels_wide × pixels_high, row-major
    pub pixels_wide: usize,      // arc-length axis
    pub pixels_high: usize,      // lateral axis
    pub arclengths: Vec<f64>,    // pixels_wide entries
    pub n_frame: Vec<[f64; 3]>,  // per-column normal vectors
    pub b_frame: Vec<[f64; 3]>,  // per-column binormal vectors
    pub positions: Vec<[f64; 3]>,// per-column centerline positions (mm)
}

/// Compute straightened CPR from centerline points (in mm).
pub fn compute_cpr(
    volume: &Array3<f32>,
    centerline_mm: &[[f64; 3]],     // N points in [z, y, x] mm
    spacing: [f64; 3],               // [sz, sy, sx]
    width_mm: f64,                   // half-width of lateral axis
    slab_mm: f64,                    // MIP slab thickness
    pixels_wide: usize,
    pixels_high: usize,
    rotation_deg: f64,
) -> CprResult {
    // 1. Fit cubic spline through centerline_mm, sample at pixels_wide positions
    // 2. Compute Bishop frame (parallel transport) at each position
    // 3. Apply rotation if rotation_deg != 0
    // 4. For each column (arc-length position):
    //    For each row (lateral offset from -width_mm to +width_mm):
    //      MIP over slab: sample volume at center ± slab/2 along binormal
    //      Use trilinear interpolation, converting mm → voxel coords
    // 5. Return CprResult
    todo!()
}

/// Bishop frame: rotation-minimizing parallel transport.
fn compute_bishop_frame(
    positions: &[Vector3<f64>],
    tangents: &[Vector3<f64>],
) -> (Vec<Vector3<f64>>, Vec<Vector3<f64>>) {
    // N[0] = initial normal perpendicular to T[0]
    // For each i: project N[i-1] onto plane perp to T[i], normalize → N[i]
    // B[i] = T[i] × N[i]
    todo!()
}
```

The implementer should refer to the Python `_compute_cpr_data()` in `/Users/shunie/Developer/PCAT/pipeline/visualize.py` for the exact algorithm. The core is:
- Bishop frame via parallel transport (vector projection + cross product)
- MIP sampling perpendicular to tangent in the (N, B) plane
- Trilinear interpolation for each sample point

- [ ] **Step 3: Create `commands/cpr.rs`**

```rust
#[tauri::command]
pub async fn compute_cpr(
    centerline_mm: Vec<[f64; 3]>,
    rotation_deg: f64,
    width_mm: f64,
    pixels_wide: usize,
    pixels_high: usize,
    state: tauri::State<'_, Mutex<AppState>>,
) -> Result<CprResponse, String> {
    let app_state = state.lock().map_err(|e| e.to_string())?;
    let vol = app_state.volume.as_ref().ok_or("No volume loaded")?;

    let result = cpr::compute_cpr(
        &vol.data, &centerline_mm, vol.spacing,
        width_mm, 3.0, pixels_wide, pixels_high, rotation_deg,
    );

    // Return image as base64-encoded raw f32 bytes + metadata
    Ok(CprResponse {
        image_b64: base64_encode(&result.image),
        shape: [result.pixels_wide, result.pixels_high],
        arclengths: result.arclengths,
    })
}

#[tauri::command]
pub async fn compute_cross_section(
    centerline_mm: Vec<[f64; 3]>,
    position_idx: usize,
    rotation_deg: f64,
    width_mm: f64,
    pixels: usize,
    state: tauri::State<'_, Mutex<AppState>>,
) -> Result<CrossSectionResponse, String> {
    // Sample volume on plane perpendicular to centerline at position_idx
    // Using trilinear interpolation
    // Return image as base64 + arc-length position
    todo!()
}
```

- [ ] **Step 4: Register commands, verify compilation**
- [ ] **Step 5: Commit**

```bash
git commit -m "feat(rust): CPR computation with Bishop frame + trilinear interpolation"
```

---

### Task 2.4: CPR Frontend Components

**Files:**
- Create: `src/components/CprView.svelte`
- Create: `src/components/CrossSection.svelte`
- Modify: `src/components/ContextPanel.svelte`

- [ ] **Step 1: Create `CprView.svelte`**

Compound panel: straightened CPR (Canvas, 70%) + 3 cross-sections (30%). Draggable needle lines. Rotation slider. Calls Rust `compute_cpr` command when centerline changes.

- [ ] **Step 2: Create `CrossSection.svelte`**

Single cross-section canvas. Calls Rust `compute_cross_section` command.

- [ ] **Step 3: Update `ContextPanel.svelte`**

Show CprView when seeds ≥ 2 for any vessel.

- [ ] **Step 4: Test**

```
cargo tauri dev → Load DICOM → Place 2+ seeds → CPR + cross-sections appear
```

- [ ] **Step 5: Commit**

```bash
git commit -m "feat: Phase 2 complete — seeds + live centerline + CPR in pure Rust"
```

---

## Chunk 3: Phase 3 — Pipeline + FAI Dashboard

### Task 3.1: Contour Extraction in Rust

**Files:**
- Create: `src-tauri/src/pipeline/contour.rs`
- Create: `src-tauri/src/pipeline/centerline.rs`

- [ ] **Step 1: Create `pipeline/centerline.rs`**

Port `clip_centerline_by_arclength()` and `estimate_vessel_radii()`:

```rust
/// Clip centerline to proximal segment [start_mm, start_mm + length_mm].
pub fn clip_by_arclength(
    centerline: &[[f64; 3]],  // voxel coords [z, y, x]
    spacing: [f64; 3],
    start_mm: f64,
    length_mm: f64,
) -> Vec<[f64; 3]> {
    // Compute cumulative arc-length, retain points in range
}

/// Estimate vessel radius at each centerline point via EDT of lumen mask.
pub fn estimate_radii(
    volume: &Array3<f32>,
    centerline: &[[f64; 3]],
    spacing: [f64; 3],
    lumen_range: (f32, f32),  // (150, 1200) HU
) -> Vec<f32> {
    // 1. Create binary lumen mask (HU in range)
    // 2. EDT from non-lumen voxels (anisotropic spacing)
    // 3. Sample EDT at each centerline point → radius
}
```

- [ ] **Step 2: Create `pipeline/contour.rs`**

Port the polar transform + gradient boundary detection:

```rust
pub struct ContourResult {
    pub r_theta: Vec<Vec<f64>>,      // [n_positions][n_angles] radii in mm
    pub r_eq: Vec<f64>,              // [n_positions] equivalent radii
    pub areas: Vec<f64>,             // [n_positions] cross-section areas mm²
    pub positions_mm: Vec<[f64; 3]>, // [n_positions] centerline positions
    pub n_frame: Vec<[f64; 3]>,      // [n_positions] normal vectors
    pub b_frame: Vec<[f64; 3]>,      // [n_positions] binormal vectors
    pub arclengths: Vec<f64>,        // [n_positions] cumulative arc-length
}

pub fn extract_contours(
    volume: &Array3<f32>,
    centerline: &[[f64; 3]],    // voxel coords
    spacing: [f64; 3],
    n_angles: usize,             // 360
    max_radius_mm: f64,          // 8.0
    sigma_deg: f64,              // 5.0
) -> ContourResult {
    // 1. Compute Bishop frame at each centerline point
    // 2. Polar sampling: for each position, for each angle, sample radially
    //    using trilinear interpolation — THIS IS THE BIG PERFORMANCE WIN
    // 3. Gradient-based boundary detection per angle
    // 4. Gaussian smoothing of r(θ) with wrap-around
    // 5. Compute area via shoelace formula, r_eq = sqrt(area/π)
}
```

The implementer should reference `/Users/shunie/Developer/PCAT/pipeline/contour_extraction.py` for the exact gradient detection logic (half-maximum descent + steepest gradient refinement).

- [ ] **Step 3: Commit**

```bash
git commit -m "feat(rust): contour extraction with polar transform + gradient detection"
```

---

### Task 3.2: VOI + FAI Stats in Rust

**Files:**
- Create: `src-tauri/src/pipeline/voi.rs`
- Create: `src-tauri/src/pipeline/stats.rs`

- [ ] **Step 1: Create `pipeline/voi.rs`**

```rust
/// Build perivascular VOI mask from contour results.
pub fn build_voi(
    volume_shape: [usize; 3],    // [Z, Y, X]
    contours: &ContourResult,
    spacing: [f64; 3],
    mode: VoiMode,               // Crisp { gap_mm, ring_mm } or Scaled { factor }
) -> Array3<bool> {
    // 1. Rasterize vessel interior from contours
    // 2. EDT from vessel boundary (anisotropic)
    // 3. Apply shell: crisp or scaled mode
}
```

- [ ] **Step 2: Create `pipeline/stats.rs`**

```rust
#[derive(Serialize)]
pub struct FaiResults {
    pub vessel: String,
    pub n_voi_voxels: usize,
    pub n_fat_voxels: usize,
    pub fat_fraction: f64,
    pub hu_mean: f64,
    pub hu_std: f64,
    pub hu_median: f64,
    pub fai_risk: String,        // "HIGH" or "LOW"
    pub histogram_bins: Vec<f64>,
    pub histogram_counts: Vec<usize>,
}

pub fn compute_pcat_stats(
    volume: &Array3<f32>,
    voi_mask: &Array3<bool>,
    vessel: &str,
    hu_range: (f64, f64),       // (-190, -30)
) -> FaiResults {
    // Extract HU values in VOI, filter to FAI range
    // Compute statistics, classify risk (threshold -70.1 HU)
}
```

- [ ] **Step 3: Commit**

```bash
git commit -m "feat(rust): VOI construction + FAI statistics computation"
```

---

### Task 3.3: Pipeline Command + Progress

**Files:**
- Create: `src-tauri/src/commands/pipeline.rs`
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Create `commands/pipeline.rs`**

```rust
#[tauri::command]
pub async fn run_pipeline(
    seeds: HashMap<String, VesselSeeds>,
    app: tauri::AppHandle,
    state: tauri::State<'_, Mutex<AppState>>,
) -> Result<PipelineResults, String> {
    // For each vessel:
    //   1. Build spline centerline from seeds
    //   2. Clip to proximal segment
    //   3. Estimate radii
    //   4. Extract contours
    //   5. Build VOI
    //   6. Compute FAI stats
    //   Emit progress events via app.emit("pipeline-progress", ...)
    // Return all results
}
```

Progress is sent to frontend via Tauri events (`app.emit()`), not SSE. The frontend listens with `listen()` from `@tauri-apps/api/event`.

- [ ] **Step 2: Register command, verify compilation**
- [ ] **Step 3: Commit**

---

### Task 3.4: FAI Dashboard Frontend

**Files:**
- Create: `src/lib/stores/pipelineStore.svelte.ts`
- Create: `src/components/AnalysisDashboard.svelte`
- Create: `src/components/ProgressOverlay.svelte`
- Modify: `src/components/ContextPanel.svelte`
- Modify: `src/App.svelte` — "Run Pipeline" button

- [ ] **Step 1: Create `pipelineStore.svelte.ts`** — listens for Tauri events
- [ ] **Step 2: Create `ProgressOverlay.svelte`** — per-vessel progress bars
- [ ] **Step 3: Create `AnalysisDashboard.svelte`** — tabs: overview, histograms (Plotly.js), CPR+FAI
- [ ] **Step 4: Add "Run Pipeline" button** to App.svelte toolbar
- [ ] **Step 5: Test end-to-end**

```
cargo tauri dev → Load DICOM → Place seeds → Run Pipeline → FAI dashboard
```

- [ ] **Step 6: Commit**

```bash
git commit -m "feat: Phase 3 complete — full pipeline + FAI dashboard in pure Rust"
```

---

## Chunk 4: Phase 4 — Polish + Distribution

### Task 4.1: Settings + Session Save/Load

- [ ] Create `SettingsDialog.svelte` (segment length, VOI mode, HU range)
- [ ] Add `commands/session.rs` — save/load session JSON via Tauri dialog
- [ ] Commit

### Task 4.2: Export

- [ ] Add export commands in Rust (PDF generation can use `printpdf` crate, .raw export is just file I/O)
- [ ] Wire export buttons in frontend
- [ ] Commit

### Task 4.3: Distribution

- [ ] Configure `tauri build` for .dmg
- [ ] Test on clean Mac
- [ ] Commit

---

## Verification Checklist

- [ ] Phase 1: Load real patient DICOM → 3 synced MPR views (Rust DICOM loading)
- [ ] Phase 2: Place seeds → live centerline + CPR (Rust CPR computation)
- [ ] Phase 3: Run Pipeline → FAI results identical to current Python app
- [ ] Phase 4: .dmg installs and runs on clean Mac, **single binary ~15MB**

---

## Rust Crate Summary

| Crate | Version | Purpose |
|---|---|---|
| `dicom` + `dicom-pixeldata` | 0.9 | DICOM file reading + pixel data extraction |
| `ndarray` | 0.16 | 3D array operations (volume storage, slicing, EDT) |
| `nalgebra` | 0.33 | 3D vector math (cross product, normalize, Bishop frame) |
| `bytemuck` | 1 | Zero-copy i16 ↔ byte conversion for slice serving |
| `walkdir` | 2 | Directory traversal for DICOM scanning |
| `tauri-plugin-dialog` | 2 | Native file/folder picker dialogs |
| `base64` | 0.22 | Encode binary data for IPC transfer |
