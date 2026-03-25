# PCAT Workstation v2 — Revised Design Spec

## Overview

Tauri v2 + Svelte 5 + Rust desktop app with Python FastAPI sidecar. Replaces the PySide6/Qt/VTK workstation. Phase 0 (scaffold) is complete.

## Layout: 田字格 (2×2 Grid)

```
┌──────────────┬──────────────┐
│    Axial     │   Coronal    │
│    (MPR)     │    (MPR)     │
├──────────────┼──────────────┤
│   Sagittal   │  Context     │
│    (MPR)     │  Panel       │
└──────────────┴──────────────┘
```

The bottom-right **Context Panel** changes by workflow stage:

| Stage | Context Panel |
|---|---|
| DICOM loaded | Patient info / empty |
| Seed placement | CPR compound view (straightened CPR + 3 cross-sections A/B/C) |
| After "Run Pipeline" | FAI dashboard (histogram, polar plot, stats table). CPR accessible via tab. |

## Pipeline Flow

```
Load DICOM  →  3 MPR views active
                    ↓
Place seeds/waypoints on MPR views (per vessel)
    → cubic spline centerline computed in frontend (instant)
    → centerline overlaid on all 3 MPR views
    → CPR view appears in context panel (Python computes CPR image)
    → 3 cross-sections at draggable A/B/C positions
                    ↓
User clicks "Run Pipeline"
    → Python: contour extraction → VOI construction → FAI stats
    → Progress via SSE
                    ↓
Context panel swaps to FAI dashboard
    → HU histogram, radial profile, stats table
    → CPR with FAI overlay accessible via tab
```

Key distinction: **seed placement is interactive** (spline + CPR only). **"Run Pipeline" is an explicit action** that triggers the compute-heavy steps (contour extraction, VOI, FAI).

## Phase 1: DICOM + MPR

### Goal
Load DICOM series via Python sidecar. Display 3 orthogonal MPR views in cornerstone3D with crosshair sync, scroll, and W/L adjustment.

### Python Sidecar Endpoints

```
POST /load_dicom {path: str}
  → Python: dicom_loader.load_dicom_series(path)
  → Saves volume as raw int16 + metadata.json to temp dir
  → Returns {volume_id, shape: [Z,Y,X], spacing: [sz,sy,sx], origin, direction, window_center, window_width}

GET /volume/{id}/slice?axis=axial&idx=N
  → Returns single slice as raw int16 bytes (for cornerstone3D loader)

POST /scan_dicom {path: str}
  → Scans directory for DICOM series (patient/study/series tree)
  → Returns series list with metadata
```

### Frontend Components

**`MprPanel.svelte`** — The 田字格 container. CSS Grid 2×2 layout with resizable splitters.

**`SliceViewport.svelte`** (×3) — Wraps a cornerstone3D `VolumeViewport`. Each instance configured for axial/coronal/sagittal orientation. Handles:
- Scroll (mouse wheel → slice index)
- W/L adjustment (right-drag)
- Crosshair sync (click → updates other views via shared Svelte store)
- Seed marker rendering (cornerstone3D annotation overlay)

**`ContextPanel.svelte`** — Bottom-right panel. Renders different content based on workflow state. Initially shows patient metadata.

**`volumeLoader.ts`** — Custom cornerstone3D image volume loader. Fetches slices from Python sidecar `GET /volume/{id}/slice`. Implements cornerstone3D `IImageVolume` interface with:
- On-demand slice loading (not full volume upfront)
- LRU cache for loaded slices
- Metadata from sidecar (spacing, origin, direction)

### Rust Backend

- `load_series` command: forwards to Python, stores volume_id + metadata in AppState
- State transitions: `Empty → DicomLoaded { volume_id, shape, spacing }`

### Milestone
Load real patient DICOM → 3 synced MPR views with scroll + W/L.

---

## Phase 2: Seeds + Live Centerline + CPR

### Goal
Click on MPR views to place ostium + waypoints per vessel. Cubic spline computed instantly in frontend. Centerline overlaid on MPR views. CPR compound view appears in context panel.

### Seed Placement (Frontend — TypeScript)

**Interaction model (matching current seed_picker.py):**
- User selects vessel (LAD/LCx/RCA) from toolbar
- Left-click on any MPR view → places seed at 3D world coordinates
- First click = ostium, subsequent clicks = waypoints
- Seeds shown as colored markers (LAD=orange, LCx=blue, RCA=green)
- Squares for ostia, circles for waypoints
- Drag to reposition, right-click to delete
- Undo/redo stack

**`seedStore.ts`** — Svelte store holding per-vessel seed data:
```typescript
type Seed = { position: [number, number, number]; type: 'ostium' | 'waypoint' };
type VesselSeeds = { vessel: 'LAD' | 'LCx' | 'RCA'; seeds: Seed[] };
```

### Spline Computation (Frontend — TypeScript)

**`spline.ts`** — Cubic spline through seed points. Runs entirely in the browser (no Python round-trip).
- Input: ostium + waypoints as 3D world coordinates (mm)
- Compute cumulative arc-length between points
- Fit cubic spline (port of scipy CubicSpline — ~100 LOC in TypeScript, or use a library)
- Sample at 0.5mm intervals
- Output: dense (N, 3) centerline array
- Recomputes instantly on any seed add/move/delete

### Centerline Overlay (Frontend)

Centerline rendered as polyline overlay on all 3 MPR views using cornerstone3D annotation tools. Updates reactively when seedStore changes.

### CPR View (Frontend + Python)

When ≥2 seeds are placed for a vessel, the context panel switches to CPR compound view.

**Python endpoint:**
```
POST /compute_cpr {centerline_mm: [[z,y,x],...], volume_id, rotation_deg, width_mm}
  → Python: _compute_cpr_data(volume, centerline_ijk, spacing_mm, rotation_deg=rotation_deg)
  → Returns CPR image as raw float32 bytes + arc-length array + frame vectors
```

**`CprView.svelte`** — The compound CPR panel matching current layout:
```
┌─────────────────────────┬──────────┐
│                         │  A (xs)  │
│   Straightened CPR      ├──────────┤
│   (vessel left→right)   │  B (xs)  │
│   3 needle lines A/B/C  ├──────────┤
│                         │  C (xs)  │
└─────────────────────────┴──────────┘
         70%                  30%
```

- Straightened CPR image rendered on HTML Canvas
- 3 draggable needle lines (A=yellow, B=cyan, C=yellow)
- Arc-length ticks at 10mm intervals
- Rotation slider in toolbar

**Cross-section panels:**
```
POST /cross_section {volume_id, centerline_mm, position_idx, rotation_deg, width_mm}
  → Returns cross-section image as raw bytes
```

- Each cross-section shows vessel lumen + optional contour overlay
- Updates when needle is dragged

**CPR recomputation:** Triggered on:
- Seed add/move/delete (new centerline → new CPR)
- Rotation angle change

### Rust Backend

- `get_seeds` / `set_seeds` commands for session persistence
- No pipeline compute in Rust — all forwarded to Python or done in frontend

### Milestone
Place seeds on MPR → see live centerline overlay + CPR with 3 cross-sections.

---

## Phase 3: Run Pipeline + FAI Dashboard

### Goal
User clicks "Run Pipeline" → Python computes contour extraction, VOI construction, FAI statistics. Results displayed in FAI dashboard.

### Pipeline Execution

**Python endpoint (SSE streaming):**
```
POST /run_pipeline {volume_id, seeds: {LAD: {...}, LCx: {...}, RCA: {...}}}
  → Streams progress via SSE:
    {"stage": "contour_extraction", "vessel": "LAD", "progress": 0.3}
    {"stage": "voi_construction", "vessel": "LAD", "progress": 1.0}
    {"stage": "fai_analysis", "vessel": "LCx", "progress": 0.5}
    ...
    {"status": "complete", "results": {...}}
```

**Python pipeline sequence (per vessel, can run in parallel across vessels):**
1. `clip_centerline_by_arclength()` — proximal segment (40mm LAD/LCx, 10-50mm RCA)
2. `estimate_vessel_radii()` — EDT-based radius estimation
3. `extract_vessel_contours()` — polar transform + gradient boundary detection
4. `build_contour_based_voi()` — contour-based perivascular shell
5. `compute_pcat_stats()` — HU filtering [-190, -30], FAI risk classification

**Results returned:**
```json
{
  "vessels": {
    "LAD": {"fai_mean_hu": -72.3, "fai_risk": "HIGH", "fat_fraction": 0.42, ...},
    "LCx": {"fai_mean_hu": -85.1, "fai_risk": "LOW", ...},
    "RCA": {"fai_mean_hu": -68.9, "fai_risk": "HIGH", ...}
  },
  "histograms": {"LAD": {"bins": [...], "counts": [...]}, ...},
  "radial_profiles": {"LAD": {"distances_mm": [...], "hu_values": [...]}, ...}
}
```

### Frontend: FAI Dashboard

**`AnalysisDashboard.svelte`** — Replaces CPR view in context panel after pipeline completes. Tabs:

**Tab 1: Overview**
- Per-vessel FAI summary cards (LAD/LCx/RCA)
- Risk classification badges (HIGH/LOW)
- Key metrics: mean HU, fat fraction, n_voxels

**Tab 2: Histograms**
- Plotly.js HU histogram per vessel
- Vertical lines at -190, -30 (FAI range), -70.1 (risk threshold)

**Tab 3: Radial Profile**
- Plotly.js radial HU profile
- Distance from vessel wall (mm) vs mean HU

**Tab 4: CPR + FAI overlay**
- Same CPR view as Phase 2, but with FAI color overlay (yellow→red)
- VOI ring shown on cross-sections

### Progress UI

**`ProgressPanel.svelte`** — Shown as overlay during pipeline execution:
- Per-vessel progress bars
- Current stage label
- Cancel button (sends abort to Python)

### Rust Backend

- `run_pipeline` command: forwards seeds to Python, streams SSE events to frontend
- State transition: `DicomLoaded → AnalysisComplete { results }`
- `cancel_pipeline` command: sends abort signal to Python

### Milestone
Place seeds → Run Pipeline → see FAI results with histograms matching current Python output.

---

## Phase 4: Polish + Distribution

### Goal
Feature parity with current Python app. Export, settings, batch mode, packaging.

### Components

**Toolbar enhancements:**
- Vessel selector dropdown (LAD/LCx/RCA/All)
- W/L presets (Soft Tissue, Lung, Bone, Custom)
- Run Pipeline button with state indication
- Export dropdown

**`SettingsDialog.svelte`:**
- Segment length (default 40mm)
- Shell thickness / VOI mode (crisp vs scaled)
- HU range for FAI
- CPR slab thickness

**Export:**
- PDF report (same layout as current `pdf_report.py`)
- .raw + metadata JSON export
- DICOM secondary capture
- All via Python sidecar endpoints

**Session persistence:**
- Save/load session as JSON (seeds, settings, results)
- Rust handles file I/O, stores session path in AppState

**Batch mode:**
- Queue multiple patient directories
- Process sequentially (or parallel via Python ProcessPool)
- Results table with per-patient summary

**Distribution:**
- `tauri build` → .dmg (macOS)
- Python sidecar bundled as PyInstaller binary
- Target: ~80-110MB total

### Milestone
Full workflow on test patient produces identical FAI values to current Python app. .dmg installs and runs on clean Mac.

---

## Tech Decisions Summary

| Decision | Choice | Rationale |
|---|---|---|
| Spline computation | Frontend (TypeScript) | Instant feedback, no round-trip. ~100 LOC port of scipy CubicSpline |
| CPR computation | Python sidecar | Needs volume data + cubic interpolation. Already implemented. |
| Contour extraction | Python sidecar | Complex algorithm (polar transform + Chan-Vese). Reuse existing code. |
| VOI + FAI | Python sidecar | Reuse existing validated code. Run per-vessel in parallel. |
| Seed state | Svelte store (frontend) | Reactive updates, instant spline recompute |
| Pipeline state | Rust (typed enum) | Compiler-enforced valid transitions |
| Cross-section images | Python sidecar | Needs volume sampling at arbitrary planes |
| Layout | 田字格 (2×2 grid) | Medical imaging standard. Context panel swaps per workflow stage. |
