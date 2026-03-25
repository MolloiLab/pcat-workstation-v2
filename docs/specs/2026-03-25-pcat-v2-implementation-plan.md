# PCAT Workstation v2 Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a Tauri v2 + Svelte 5 desktop app for pericoronary adipose tissue (PCAT/FAI) analysis, replacing the current PySide6/Qt/VTK workstation.

**Architecture:** 田字格 (2×2) layout: 3 MPR viewports + 1 context panel (CPR or FAI dashboard). Rust backend manages state + Python sidecar lifecycle. Python FastAPI sidecar handles DICOM loading, CPR computation, and pipeline execution. Frontend handles seed placement + spline computation for instant feedback.

**Tech Stack:** Tauri v2, Svelte 5, TypeScript, cornerstone3D, Plotly.js, Tailwind CSS v4, Rust, Python FastAPI, numpy/scipy/pydicom.

**Spec:** `docs/specs/2026-03-25-pcat-v2-revised-design.md`

**Existing code (Phase 0 complete):**
- `src/App.svelte` — shell with toolbar, sidebar, status bar
- `src-tauri/src/lib.rs` — Tauri setup, `ping_sidecar` + `get_pipeline_state` commands
- `src-tauri/src/state.rs` — `PipelineState` enum, `Vessel`, `Seed`, `AppState`
- `src-tauri/src/sidecar.rs` — `SidecarManager` (spawn Python, read port, health check)
- `pipeline_server/main.py` — FastAPI with `/ping` + stub endpoints

---

## File Structure

### Frontend (`src/`)

```
src/
├── main.ts                          # Svelte mount (exists)
├── app.css                          # Tailwind + theme (exists)
├── App.svelte                       # Shell layout (modify — add 田字格)
├── lib/
│   ├── stores/
│   │   ├── volumeStore.ts           # Volume metadata + loading state
│   │   ├── seedStore.ts             # Per-vessel seed data, reactive
│   │   ├── viewportStore.ts         # Crosshair position, W/L, scroll indices
│   │   └── pipelineStore.ts         # Pipeline state, results, progress
│   ├── cornerstone/
│   │   ├── init.ts                  # cornerstone3D + tools initialization
│   │   ├── volumeLoader.ts          # Custom image volume loader (fetch slices from Python)
│   │   └── tools.ts                 # Tool group setup (W/L, scroll, crosshairs)
│   ├── spline.ts                    # Cubic spline interpolation (TypeScript port)
│   └── api.ts                       # HTTP helpers for Python sidecar calls
├── components/
│   ├── MprPanel.svelte              # 田字格 2×2 grid container
│   ├── SliceViewport.svelte         # Single cornerstone3D viewport wrapper
│   ├── ContextPanel.svelte          # Bottom-right panel (swaps content by state)
│   ├── CprView.svelte               # Compound CPR: straightened CPR + 3 cross-sections
│   ├── CrossSection.svelte          # Single vessel cross-section canvas
│   ├── SeedToolbar.svelte           # Vessel selector + seed mode controls
│   ├── AnalysisDashboard.svelte     # FAI results: tabs for overview/histogram/radial/CPR
│   ├── ProgressOverlay.svelte       # Pipeline progress bars overlay
│   ├── Toolbar.svelte               # Top toolbar (exists conceptually in App.svelte, extract)
│   └── StatusBar.svelte             # Bottom status bar (exists conceptually, extract)
```

### Rust Backend (`src-tauri/src/`)

```
src-tauri/src/
├── main.rs                          # Binary entry (exists)
├── lib.rs                           # Tauri setup + commands (modify)
├── state.rs                         # AppState + PipelineState (modify)
├── sidecar.rs                       # SidecarManager (exists, minor tweaks)
└── commands.rs                      # All Tauri commands (extract from lib.rs)
```

### Python Sidecar (`pipeline_server/`)

```
pipeline_server/
├── main.py                          # FastAPI app + startup (modify)
├── dicom_routes.py                  # /scan_dicom, /load_dicom, /volume/{id}/slice
├── cpr_routes.py                    # /compute_cpr, /cross_section
├── pipeline_routes.py               # /run_pipeline (SSE), /cancel_pipeline
├── volume_manager.py                # In-memory volume storage + slice serving
└── requirements.txt                 # Dependencies (exists, update)
```

---

## Chunk 1: Phase 1 — DICOM + MPR

### Task 1.1: Python DICOM Loading Endpoints

**Files:**
- Create: `pipeline_server/dicom_routes.py`
- Create: `pipeline_server/volume_manager.py`
- Modify: `pipeline_server/main.py`

This task wires the existing `pipeline/dicom_loader.py` into the FastAPI server so the frontend can load DICOM data.

- [ ] **Step 1: Create `volume_manager.py`**

Manages loaded volumes in memory. Stores raw numpy arrays keyed by volume_id. Serves individual slices.

```python
# pipeline_server/volume_manager.py
import numpy as np
import uuid
from dataclasses import dataclass, field
from typing import Dict, Optional, Tuple

@dataclass
class LoadedVolume:
    volume_id: str
    data: np.ndarray              # (Z, Y, X) int16
    spacing: Tuple[float, float, float]  # (sz, sy, sx)
    origin: Tuple[float, float, float]
    direction: Tuple[float, ...]  # 9 elements, row-major 3×3
    window_center: float
    window_width: float
    patient_name: str = ""
    study_description: str = ""

class VolumeManager:
    def __init__(self):
        self._volumes: Dict[str, LoadedVolume] = {}

    def store(self, vol: LoadedVolume) -> str:
        self._volumes[vol.volume_id] = vol
        return vol.volume_id

    def get(self, volume_id: str) -> Optional[LoadedVolume]:
        return self._volumes.get(volume_id)

    def get_slice(self, volume_id: str, axis: str, idx: int) -> Optional[bytes]:
        vol = self._volumes.get(volume_id)
        if vol is None:
            return None
        data = vol.data
        if axis == "axial":
            idx = min(max(0, idx), data.shape[0] - 1)
            slc = data[idx, :, :]
        elif axis == "coronal":
            idx = min(max(0, idx), data.shape[1] - 1)
            slc = data[:, idx, :]
        elif axis == "sagittal":
            idx = min(max(0, idx), data.shape[2] - 1)
            slc = data[:, :, idx]
        else:
            return None
        return slc.astype(np.int16).tobytes()

    def remove(self, volume_id: str):
        self._volumes.pop(volume_id, None)

volumes = VolumeManager()
```

- [ ] **Step 2: Create `dicom_routes.py`**

```python
# pipeline_server/dicom_routes.py
import sys, os, uuid
import numpy as np
from fastapi import APIRouter, HTTPException
from pydantic import BaseModel
from typing import List, Optional
from fastapi.responses import Response

# Add the PCAT pipeline to Python path so we can import dicom_loader
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "..", "pipeline"))

from volume_manager import volumes, LoadedVolume

router = APIRouter()

class ScanRequest(BaseModel):
    path: str

class SeriesInfo(BaseModel):
    series_uid: str
    description: str
    modality: str
    num_images: int
    patient_name: str
    study_description: str

class LoadRequest(BaseModel):
    path: str
    series_uid: Optional[str] = None

@router.post("/scan_dicom")
async def scan_dicom(req: ScanRequest):
    """Scan directory for DICOM series."""
    import pydicom
    from pathlib import Path

    series_map = {}
    scan_path = Path(req.path)
    if not scan_path.exists():
        raise HTTPException(404, f"Path not found: {req.path}")

    for dcm_path in scan_path.rglob("*"):
        if dcm_path.is_dir():
            continue
        try:
            ds = pydicom.dcmread(str(dcm_path), stop_before_pixels=True, force=True)
            uid = getattr(ds, "SeriesInstanceUID", None)
            if uid is None:
                continue
            uid = str(uid)
            if uid not in series_map:
                series_map[uid] = {
                    "series_uid": uid,
                    "description": getattr(ds, "SeriesDescription", ""),
                    "modality": getattr(ds, "Modality", ""),
                    "num_images": 0,
                    "patient_name": str(getattr(ds, "PatientName", "")),
                    "study_description": getattr(ds, "StudyDescription", ""),
                }
            series_map[uid]["num_images"] += 1
        except Exception:
            continue

    return {"series": list(series_map.values())}

@router.post("/load_dicom")
async def load_dicom(req: LoadRequest):
    """Load DICOM series into memory."""
    from dicom_loader import load_dicom_series

    try:
        volume, meta = load_dicom_series(req.path)
    except Exception as e:
        raise HTTPException(500, f"Failed to load DICOM: {e}")

    volume_id = str(uuid.uuid4())[:8]
    spacing = tuple(float(s) for s in meta.get("spacing_mm", [1.0, 1.0, 1.0]))
    origin = tuple(float(o) for o in meta.get("origin", [0.0, 0.0, 0.0]))
    direction = tuple(float(d) for d in meta.get("direction", [1,0,0,0,1,0,0,0,1]))

    vol = LoadedVolume(
        volume_id=volume_id,
        data=volume.astype(np.int16),
        spacing=spacing,
        origin=origin,
        direction=direction,
        window_center=float(meta.get("window_center", 40)),
        window_width=float(meta.get("window_width", 400)),
        patient_name=str(meta.get("patient_name", "")),
        study_description=str(meta.get("study_description", "")),
    )
    volumes.store(vol)

    return {
        "volume_id": volume_id,
        "shape": list(volume.shape),
        "spacing": list(spacing),
        "origin": list(origin),
        "direction": list(direction),
        "window_center": vol.window_center,
        "window_width": vol.window_width,
        "patient_name": vol.patient_name,
        "study_description": vol.study_description,
    }

@router.get("/volume/{volume_id}/slice")
async def get_slice(volume_id: str, axis: str = "axial", idx: int = 0):
    """Return a single slice as raw int16 bytes."""
    data = volumes.get_slice(volume_id, axis, idx)
    if data is None:
        raise HTTPException(404, "Volume or slice not found")
    return Response(content=data, media_type="application/octet-stream")
```

- [ ] **Step 3: Wire routes into `main.py`**

Add `from dicom_routes import router as dicom_router` and `app.include_router(dicom_router)`. Remove the old stub endpoints for scan/load/slice.

- [ ] **Step 4: Test DICOM loading manually**

```bash
# Start sidecar
python3 pipeline_server/main.py &
# In another terminal, test with a real DICOM directory:
curl -X POST http://127.0.0.1:PORT/scan_dicom -H 'Content-Type: application/json' -d '{"path": "/path/to/dicom"}'
curl -X POST http://127.0.0.1:PORT/load_dicom -H 'Content-Type: application/json' -d '{"path": "/path/to/dicom"}'
curl "http://127.0.0.1:PORT/volume/VOLUME_ID/slice?axis=axial&idx=100" --output slice.raw
```

- [ ] **Step 5: Commit**

```bash
git add pipeline_server/
git commit -m "feat(sidecar): add DICOM loading endpoints with volume manager"
```

---

### Task 1.2: Install cornerstone3D + Frontend Init

**Files:**
- Create: `src/lib/cornerstone/init.ts`
- Create: `src/lib/cornerstone/tools.ts`
- Create: `src/lib/api.ts`
- Modify: `package.json`

- [ ] **Step 1: Install cornerstone3D packages**

```bash
npm install @cornerstonejs/core @cornerstonejs/tools @cornerstonejs/streaming-image-volume-loader dicom-parser
```

Note: cornerstone3D v2 may have different package names. Check latest docs. The core packages needed are the rendering engine, tools (W/L, scroll, crosshairs), and a way to create volumes.

- [ ] **Step 2: Create `src/lib/cornerstone/init.ts`**

Initialize cornerstone3D rendering engine and register tools. This runs once on app startup.

```typescript
// src/lib/cornerstone/init.ts
import { init as csInit, RenderingEngine, Enums } from '@cornerstonejs/core';
import { init as csToolsInit, addTool, ToolGroupManager } from '@cornerstonejs/tools';
import {
  WindowLevelTool,
  StackScrollTool,
  CrosshairsTool,
  PanTool,
  ZoomTool,
} from '@cornerstonejs/tools';

let renderingEngine: RenderingEngine | null = null;

export async function initCornerstone(): Promise<RenderingEngine> {
  if (renderingEngine) return renderingEngine;

  await csInit();
  await csToolsInit();

  addTool(WindowLevelTool);
  addTool(StackScrollTool);
  addTool(CrosshairsTool);
  addTool(PanTool);
  addTool(ZoomTool);

  renderingEngine = new RenderingEngine('pcat-engine');
  return renderingEngine;
}

export function getRenderingEngine(): RenderingEngine {
  if (!renderingEngine) throw new Error('Cornerstone not initialized');
  return renderingEngine;
}
```

- [ ] **Step 3: Create `src/lib/api.ts`**

```typescript
// src/lib/api.ts
import { invoke } from '@tauri-apps/api/core';

let sidecarPort: number | null = null;

export async function getSidecarPort(): Promise<number> {
  if (sidecarPort) return sidecarPort;
  const result = await invoke<{ port: number }>('get_sidecar_port');
  if (!result) throw new Error('Sidecar not ready');
  sidecarPort = result;
  return sidecarPort;
}

export async function sidecarFetch(path: string, options?: RequestInit): Promise<Response> {
  const port = await getSidecarPort();
  return fetch(`http://127.0.0.1:${port}${path}`, options);
}

export async function sidecarJson<T>(path: string, options?: RequestInit): Promise<T> {
  const resp = await sidecarFetch(path, options);
  if (!resp.ok) throw new Error(`Sidecar error: ${resp.status} ${await resp.text()}`);
  return resp.json();
}

export async function sidecarPost<T>(path: string, body: unknown): Promise<T> {
  return sidecarJson<T>(path, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
}
```

- [ ] **Step 4: Create `src/lib/cornerstone/tools.ts`**

```typescript
// src/lib/cornerstone/tools.ts
import { ToolGroupManager, Enums as ToolEnums } from '@cornerstonejs/tools';
import {
  WindowLevelTool,
  StackScrollTool,
  CrosshairsTool,
  PanTool,
  ZoomTool,
} from '@cornerstonejs/tools';

const TOOL_GROUP_ID = 'pcat-mpr-tools';

export function setupToolGroup(viewportIds: string[]): void {
  const toolGroup = ToolGroupManager.createToolGroup(TOOL_GROUP_ID);
  if (!toolGroup) return;

  toolGroup.addTool(WindowLevelTool.toolName);
  toolGroup.addTool(PanTool.toolName);
  toolGroup.addTool(ZoomTool.toolName);
  toolGroup.addTool(StackScrollTool.toolName);
  toolGroup.addTool(CrosshairsTool.toolName);

  // Right-drag = W/L, Middle-drag = Pan, Scroll = Stack scroll
  toolGroup.setToolActive(WindowLevelTool.toolName, { bindings: [{ mouseButton: ToolEnums.MouseBindings.Secondary }] });
  toolGroup.setToolActive(PanTool.toolName, { bindings: [{ mouseButton: ToolEnums.MouseBindings.Auxiliary }] });
  toolGroup.setToolActive(ZoomTool.toolName, { bindings: [{ mouseButton: ToolEnums.MouseBindings.Primary, modifierKey: ToolEnums.KeyboardBindings.Ctrl }] });
  toolGroup.setToolActive(StackScrollTool.toolName, { bindings: [{ mouseButton: ToolEnums.MouseBindings.Wheel }] });
  toolGroup.setToolActive(CrosshairsTool.toolName, { bindings: [{ mouseButton: ToolEnums.MouseBindings.Primary }] });

  for (const id of viewportIds) {
    toolGroup.addViewport(id, 'pcat-engine');
  }
}
```

- [ ] **Step 5: Commit**

```bash
git add src/lib/ package.json package-lock.json
git commit -m "feat(frontend): add cornerstone3D init, tools, and sidecar API helpers"
```

---

### Task 1.3: Volume Loader + Stores

**Files:**
- Create: `src/lib/cornerstone/volumeLoader.ts`
- Create: `src/lib/stores/volumeStore.ts`
- Create: `src/lib/stores/viewportStore.ts`

- [ ] **Step 1: Create `src/lib/stores/volumeStore.ts`**

```typescript
// src/lib/stores/volumeStore.ts

export type VolumeMetadata = {
  volumeId: string;
  shape: [number, number, number]; // [Z, Y, X]
  spacing: [number, number, number]; // [sz, sy, sx]
  origin: [number, number, number];
  direction: number[];
  windowCenter: number;
  windowWidth: number;
  patientName: string;
  studyDescription: string;
};

let currentVolume: VolumeMetadata | null = $state(null);

export const volumeStore = {
  get current() { return currentVolume; },
  set(vol: VolumeMetadata) { currentVolume = vol; },
  clear() { currentVolume = null; },
};
```

- [ ] **Step 2: Create `src/lib/stores/viewportStore.ts`**

```typescript
// src/lib/stores/viewportStore.ts

export type CrosshairPosition = {
  worldX: number;
  worldY: number;
  worldZ: number;
};

let crosshair: CrosshairPosition = $state({ worldX: 0, worldY: 0, worldZ: 0 });

export const viewportStore = {
  get crosshair() { return crosshair; },
  setCrosshair(pos: CrosshairPosition) { crosshair = pos; },
};
```

- [ ] **Step 3: Create `src/lib/cornerstone/volumeLoader.ts`**

Custom volume loader that fetches slices from the Python sidecar. cornerstone3D needs an `IImageVolume` with the full voxel data. Since we're loading from a non-DICOM source (raw int16 from Python), we register a custom `imageLoader` and build the volume from fetched slices.

```typescript
// src/lib/cornerstone/volumeLoader.ts
import { volumeLoader, Enums, cache, imageLoader } from '@cornerstonejs/core';
import type { Types } from '@cornerstonejs/core';
import { sidecarFetch } from '../api';
import type { VolumeMetadata } from '../stores/volumeStore';

const SCHEME = 'pcatVolume';

/**
 * Load the full volume from the Python sidecar slice-by-slice,
 * then create a cornerstone3D StreamingImageVolume.
 */
export async function loadVolume(
  meta: VolumeMetadata,
  onProgress?: (loaded: number, total: number) => void,
): Promise<string> {
  const volumeId = `${SCHEME}:${meta.volumeId}`;
  const [Z, Y, X] = meta.shape;
  const totalSlices = Z;

  // Allocate the full volume buffer
  const pixelData = new Int16Array(Z * Y * X);

  // Fetch all axial slices
  for (let z = 0; z < totalSlices; z++) {
    const resp = await sidecarFetch(
      `/volume/${meta.volumeId}/slice?axis=axial&idx=${z}`
    );
    const buf = await resp.arrayBuffer();
    const slice = new Int16Array(buf);
    pixelData.set(slice, z * Y * X);
    if (onProgress) onProgress(z + 1, totalSlices);
  }

  // Create the volume in cornerstone3D cache
  const volume = await volumeLoader.createAndCacheVolume(volumeId, {
    dimensions: [X, Y, Z],
    spacing: [meta.spacing[2], meta.spacing[1], meta.spacing[0]], // cornerstone uses [x,y,z]
    origin: [meta.origin[2], meta.origin[1], meta.origin[0]],
    direction: meta.direction,
    scalarData: pixelData,
    metadata: {
      BitsAllocated: 16,
      BitsStored: 16,
      SamplesPerPixel: 1,
      HighBit: 15,
      PixelRepresentation: 1, // signed
      PhotometricInterpretation: 'MONOCHROME2',
      Modality: 'CT',
    },
  });

  return volumeId;
}
```

Note: The exact cornerstone3D API may differ between versions. The implementer should consult cornerstone3D v2 docs for `createAndCacheVolume`. The concept is: allocate Int16Array, fill from sidecar slices, register as volume.

- [ ] **Step 4: Commit**

```bash
git add src/lib/
git commit -m "feat(frontend): add volume loader, volume store, viewport store"
```

---

### Task 1.4: MPR Panel (田字格 Layout)

**Files:**
- Create: `src/components/MprPanel.svelte`
- Create: `src/components/SliceViewport.svelte`
- Create: `src/components/ContextPanel.svelte`
- Modify: `src/App.svelte`

- [ ] **Step 1: Create `SliceViewport.svelte`**

Wraps a single cornerstone3D `VolumeViewport`. Takes `orientation` prop (axial/coronal/sagittal) and a `viewportId`.

```svelte
<!-- src/components/SliceViewport.svelte -->
<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { getRenderingEngine } from '../lib/cornerstone/init';
  import { Enums } from '@cornerstonejs/core';

  type Orientation = 'axial' | 'coronal' | 'sagittal';

  let { orientation, viewportId, volumeId }: {
    orientation: Orientation;
    viewportId: string;
    volumeId: string | null;
  } = $props();

  let container: HTMLDivElement;

  const orientationMap: Record<Orientation, string> = {
    axial: Enums.OrientationAxis.AXIAL,
    coronal: Enums.OrientationAxis.CORONAL,
    sagittal: Enums.OrientationAxis.SAGITTAL,
  };

  onMount(() => {
    const engine = getRenderingEngine();
    engine.enableElement({
      viewportId,
      type: Enums.ViewportType.ORTHOGRAPHIC,
      element: container,
      defaultOptions: {
        orientation: orientationMap[orientation],
      },
    });

    if (volumeId) {
      const viewport = engine.getViewport(viewportId);
      viewport.setVolumes([{ volumeId }]);
      viewport.render();
    }
  });

  onDestroy(() => {
    const engine = getRenderingEngine();
    engine.disableElement(viewportId);
  });

  // React to volumeId changes
  $effect(() => {
    if (volumeId && container) {
      const engine = getRenderingEngine();
      const viewport = engine.getViewport(viewportId);
      if (viewport) {
        viewport.setVolumes([{ volumeId }]);
        viewport.render();
      }
    }
  });
</script>

<div bind:this={container} class="h-full w-full bg-black"></div>
```

- [ ] **Step 2: Create `ContextPanel.svelte`**

```svelte
<!-- src/components/ContextPanel.svelte -->
<script lang="ts">
  import { volumeStore } from '../lib/stores/volumeStore';

  // Later phases add CprView and AnalysisDashboard imports here
  let { phase }: { phase: 'empty' | 'dicom' | 'seeds' | 'analysis' } = $props();
</script>

{#if phase === 'empty' || phase === 'dicom'}
  <div class="flex h-full items-center justify-center text-text-secondary">
    {#if volumeStore.current}
      <div class="text-center">
        <p class="text-sm">{volumeStore.current.patientName}</p>
        <p class="text-xs mt-1">{volumeStore.current.studyDescription}</p>
        <p class="text-xs mt-1">
          {volumeStore.current.shape[0]}×{volumeStore.current.shape[1]}×{volumeStore.current.shape[2]}
          — {volumeStore.current.spacing.map(s => s.toFixed(2)).join('×')} mm
        </p>
      </div>
    {:else}
      <p class="text-sm">No DICOM loaded</p>
    {/if}
  </div>
{/if}
```

- [ ] **Step 3: Create `MprPanel.svelte`**

```svelte
<!-- src/components/MprPanel.svelte -->
<script lang="ts">
  import SliceViewport from './SliceViewport.svelte';
  import ContextPanel from './ContextPanel.svelte';

  let { volumeId }: { volumeId: string | null } = $props();
</script>

<div class="grid h-full w-full grid-cols-2 grid-rows-2 gap-px bg-border">
  <div class="bg-surface">
    <SliceViewport orientation="axial" viewportId="vp-axial" {volumeId} />
  </div>
  <div class="bg-surface">
    <SliceViewport orientation="coronal" viewportId="vp-coronal" {volumeId} />
  </div>
  <div class="bg-surface">
    <SliceViewport orientation="sagittal" viewportId="vp-sagittal" {volumeId} />
  </div>
  <div class="bg-surface-secondary">
    <ContextPanel phase={volumeId ? 'dicom' : 'empty'} />
  </div>
</div>
```

- [ ] **Step 4: Rewrite `App.svelte` with 田字格**

Replace the current sidebar+viewport layout with the 田字格. Keep toolbar + status bar. Add "Open DICOM" button wiring.

The implementer should:
1. Remove the sidebar from the current `App.svelte`
2. Replace the viewport area with `<MprPanel volumeId={volumeId} />`
3. Wire the "Open DICOM" button to: call `invoke('open_dicom_dialog')` → get path → call `sidecarPost('/load_dicom', {path})` → store metadata in `volumeStore` → call `loadVolume()` → set `volumeId`
4. Show loading progress during volume fetch

- [ ] **Step 5: Add Rust `open_dicom_dialog` command**

In `src-tauri/src/lib.rs`, add a command that opens a native folder picker dialog (via `tauri::dialog`) and returns the selected path. This lets the user pick a DICOM directory.

- [ ] **Step 6: Update Rust `get_sidecar_port` command**

Ensure `get_sidecar_port` returns just the port number (not wrapped in an object) so `api.ts` can use it directly.

- [ ] **Step 7: Test end-to-end**

```
cargo tauri dev → Open DICOM → pick folder → 3 MPR views render with scroll + W/L
```

- [ ] **Step 8: Commit**

```bash
git add src/ src-tauri/
git commit -m "feat: Phase 1 — DICOM loading + 田字格 MPR views with cornerstone3D"
```

---

## Chunk 2: Phase 2 — Seeds + Live Centerline + CPR

### Task 2.1: Seed Store + Spline Computation

**Files:**
- Create: `src/lib/stores/seedStore.ts`
- Create: `src/lib/spline.ts`

- [ ] **Step 1: Create `src/lib/stores/seedStore.ts`**

```typescript
// src/lib/stores/seedStore.ts
import { computeSplineCenterline } from '../spline';

export type Vessel = 'LAD' | 'LCx' | 'RCA';
export type SeedType = 'ostium' | 'waypoint';
export type Seed = { position: [number, number, number]; type: SeedType };

export type VesselData = {
  seeds: Seed[];
  centerline: [number, number, number][] | null; // dense spline points in mm
};

const VESSELS: Vessel[] = ['LAD', 'LCx', 'RCA'];
const VESSEL_COLORS: Record<Vessel, string> = { LAD: '#ff8c00', LCx: '#4488ff', RCA: '#44cc44' };

let activeVessel: Vessel = $state('LAD');
let vesselData: Record<Vessel, VesselData> = $state({
  LAD: { seeds: [], centerline: null },
  LCx: { seeds: [], centerline: null },
  RCA: { seeds: [], centerline: null },
});

function recomputeCenterline(vessel: Vessel) {
  const data = vesselData[vessel];
  if (data.seeds.length < 2) {
    data.centerline = null;
    return;
  }
  const points = data.seeds.map(s => s.position);
  data.centerline = computeSplineCenterline(points, 0.5);
}

export const seedStore = {
  get activeVessel() { return activeVessel; },
  setActiveVessel(v: Vessel) { activeVessel = v; },
  get vessels() { return VESSELS; },
  get colors() { return VESSEL_COLORS; },

  getData(vessel: Vessel) { return vesselData[vessel]; },
  get activeData() { return vesselData[activeVessel]; },

  addSeed(position: [number, number, number]) {
    const data = vesselData[activeVessel];
    const type: SeedType = data.seeds.length === 0 ? 'ostium' : 'waypoint';
    data.seeds = [...data.seeds, { position, type }];
    recomputeCenterline(activeVessel);
  },

  removeSeed(index: number) {
    const data = vesselData[activeVessel];
    data.seeds = data.seeds.filter((_, i) => i !== index);
    // Reassign first seed as ostium if needed
    if (data.seeds.length > 0) data.seeds[0].type = 'ostium';
    recomputeCenterline(activeVessel);
  },

  moveSeed(index: number, position: [number, number, number]) {
    const data = vesselData[activeVessel];
    data.seeds = data.seeds.map((s, i) => i === index ? { ...s, position } : s);
    recomputeCenterline(activeVessel);
  },

  clearVessel(vessel: Vessel) {
    vesselData[vessel] = { seeds: [], centerline: null };
  },

  clearAll() {
    for (const v of VESSELS) vesselData[v] = { seeds: [], centerline: null };
  },
};
```

- [ ] **Step 2: Create `src/lib/spline.ts`**

Port of scipy's CubicSpline with natural boundary conditions. ~100 LOC.

```typescript
// src/lib/spline.ts

/**
 * Natural cubic spline interpolation through 3D points.
 * Returns dense array of points sampled at `stepMm` mm intervals.
 */
export function computeSplineCenterline(
  points: [number, number, number][],
  stepMm: number = 0.5,
): [number, number, number][] {
  if (points.length < 2) return [];
  if (points.length === 2) return interpolateLinear(points[0], points[1], stepMm);

  const n = points.length;
  // Compute cumulative arc-length
  const arcLengths = [0];
  for (let i = 1; i < n; i++) {
    const dx = points[i][0] - points[i - 1][0];
    const dy = points[i][1] - points[i - 1][1];
    const dz = points[i][2] - points[i - 1][2];
    arcLengths.push(arcLengths[i - 1] + Math.sqrt(dx * dx + dy * dy + dz * dz));
  }
  const totalArc = arcLengths[n - 1];
  if (totalArc < 1e-6) return [points[0]];

  // Fit natural cubic spline per dimension
  const splineX = fitNaturalCubicSpline(arcLengths, points.map(p => p[0]));
  const splineY = fitNaturalCubicSpline(arcLengths, points.map(p => p[1]));
  const splineZ = fitNaturalCubicSpline(arcLengths, points.map(p => p[2]));

  // Sample at stepMm intervals
  const nSamples = Math.max(2, Math.ceil(totalArc / stepMm) + 1);
  const result: [number, number, number][] = [];
  for (let i = 0; i < nSamples; i++) {
    const s = (i / (nSamples - 1)) * totalArc;
    result.push([evalSpline(splineX, arcLengths, s), evalSpline(splineY, arcLengths, s), evalSpline(splineZ, arcLengths, s)]);
  }
  return result;
}

function interpolateLinear(
  a: [number, number, number],
  b: [number, number, number],
  stepMm: number,
): [number, number, number][] {
  const dx = b[0] - a[0], dy = b[1] - a[1], dz = b[2] - a[2];
  const dist = Math.sqrt(dx * dx + dy * dy + dz * dz);
  const n = Math.max(2, Math.ceil(dist / stepMm) + 1);
  const result: [number, number, number][] = [];
  for (let i = 0; i < n; i++) {
    const t = i / (n - 1);
    result.push([a[0] + dx * t, a[1] + dy * t, a[2] + dz * t]);
  }
  return result;
}

type SplineCoeffs = { a: number[]; b: number[]; c: number[]; d: number[] };

function fitNaturalCubicSpline(t: number[], y: number[]): SplineCoeffs {
  const n = t.length - 1;
  const h = Array(n);
  for (let i = 0; i < n; i++) h[i] = t[i + 1] - t[i];

  // Solve tridiagonal system for second derivatives (natural BCs: c[0]=c[n]=0)
  const alpha = Array(n + 1).fill(0);
  for (let i = 1; i < n; i++) {
    alpha[i] = (3 / h[i]) * (y[i + 1] - y[i]) - (3 / h[i - 1]) * (y[i] - y[i - 1]);
  }

  const l = Array(n + 1).fill(1);
  const mu = Array(n + 1).fill(0);
  const z = Array(n + 1).fill(0);

  for (let i = 1; i < n; i++) {
    l[i] = 2 * (t[i + 1] - t[i - 1]) - h[i - 1] * mu[i - 1];
    mu[i] = h[i] / l[i];
    z[i] = (alpha[i] - h[i - 1] * z[i - 1]) / l[i];
  }

  const c = Array(n + 1).fill(0);
  const b = Array(n).fill(0);
  const d = Array(n).fill(0);

  for (let j = n - 1; j >= 0; j--) {
    c[j] = z[j] - mu[j] * c[j + 1];
    b[j] = (y[j + 1] - y[j]) / h[j] - h[j] * (c[j + 1] + 2 * c[j]) / 3;
    d[j] = (c[j + 1] - c[j]) / (3 * h[j]);
  }

  return { a: y.slice(0, n), b, c: c.slice(0, n), d };
}

function evalSpline(sp: SplineCoeffs, t: number[], s: number): number {
  // Find interval
  let i = sp.a.length - 1;
  for (let j = 0; j < sp.a.length; j++) {
    if (s <= t[j + 1]) { i = j; break; }
  }
  const ds = s - t[i];
  return sp.a[i] + sp.b[i] * ds + sp.c[i] * ds * ds + sp.d[i] * ds * ds * ds;
}
```

- [ ] **Step 3: Commit**

```bash
git add src/lib/
git commit -m "feat: seed store with reactive spline centerline computation"
```

---

### Task 2.2: Seed Placement on MPR Views

**Files:**
- Create: `src/components/SeedToolbar.svelte`
- Modify: `src/components/SliceViewport.svelte` — add click handler for seed placement
- Modify: `src/App.svelte` — add SeedToolbar

- [ ] **Step 1: Create `SeedToolbar.svelte`**

Vessel selector buttons (LAD/LCx/RCA) + seed count display + clear button.

```svelte
<!-- src/components/SeedToolbar.svelte -->
<script lang="ts">
  import { seedStore, type Vessel } from '../lib/stores/seedStore';
</script>

<div class="flex items-center gap-2">
  {#each seedStore.vessels as vessel}
    {@const data = seedStore.getData(vessel)}
    {@const isActive = seedStore.activeVessel === vessel}
    <button
      class="rounded px-3 py-1 text-xs font-medium transition-colors"
      style="color: {isActive ? '#fff' : seedStore.colors[vessel]}; background: {isActive ? seedStore.colors[vessel] : 'transparent'}; border: 1px solid {seedStore.colors[vessel]}"
      onclick={() => seedStore.setActiveVessel(vessel)}
    >
      {vessel} ({data.seeds.length})
    </button>
  {/each}
  <button
    class="ml-2 rounded px-2 py-1 text-xs text-text-secondary hover:text-error"
    onclick={() => seedStore.clearAll()}
  >
    Clear All
  </button>
</div>
```

- [ ] **Step 2: Add click handler to `SliceViewport.svelte`**

When user left-clicks on the viewport, convert canvas coordinates to 3D world coordinates and call `seedStore.addSeed()`. The implementer should use cornerstone3D's `viewport.canvasToWorld(canvasPoint)` API.

Key implementation: listen for cornerstone3D `MOUSE_CLICK` event on the viewport element, extract world coordinates, call `seedStore.addSeed([worldX, worldY, worldZ])`.

- [ ] **Step 3: Render seed markers as overlays**

Use cornerstone3D `AnnotationTool` or SVG overlay to render seed markers on each viewport. Markers should be reactive to `seedStore` changes.

For each seed, project the 3D world position onto the viewport's current slice. If the seed is within ±1 slice of the current view, render it as a colored marker.

- [ ] **Step 4: Render centerline overlay**

When `seedStore.activeData.centerline` is not null, render it as a polyline on each viewport. Use cornerstone3D annotation or SVG overlay. Project each 3D point onto the viewport's image plane.

- [ ] **Step 5: Wire SeedToolbar into App.svelte**

Add `<SeedToolbar />` to the toolbar area in App.svelte.

- [ ] **Step 6: Test**

```
cargo tauri dev → Load DICOM → Click on MPR views → Seeds appear → Centerline draws through them
```

- [ ] **Step 7: Commit**

```bash
git add src/
git commit -m "feat: Phase 2a — seed placement on MPR views with live spline centerline"
```

---

### Task 2.3: CPR View (Compound Panel)

**Files:**
- Create: `src/components/CprView.svelte`
- Create: `src/components/CrossSection.svelte`
- Create: `pipeline_server/cpr_routes.py`
- Modify: `pipeline_server/main.py` — include CPR routes
- Modify: `src/components/ContextPanel.svelte` — show CPR when seeds placed

- [ ] **Step 1: Create Python CPR endpoints**

```python
# pipeline_server/cpr_routes.py
import sys, os, json
import numpy as np
from fastapi import APIRouter, HTTPException
from fastapi.responses import Response
from pydantic import BaseModel
from typing import List, Optional

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "..", "pipeline"))

from volume_manager import volumes

router = APIRouter()

class CprRequest(BaseModel):
    volume_id: str
    centerline_mm: List[List[float]]  # [[z,y,x], ...] in mm
    rotation_deg: float = 0.0
    width_mm: float = 25.0
    pixels_wide: int = 512
    pixels_high: int = 256

class CrossSectionRequest(BaseModel):
    volume_id: str
    centerline_mm: List[List[float]]
    position_idx: int
    rotation_deg: float = 0.0
    width_mm: float = 15.0
    pixels: int = 128

@router.post("/compute_cpr")
async def compute_cpr(req: CprRequest):
    """Compute straightened CPR image from centerline."""
    from visualize import _compute_cpr_data

    vol = volumes.get(req.volume_id)
    if not vol:
        raise HTTPException(404, "Volume not found")

    centerline_ijk = np.array(req.centerline_mm) / np.array(vol.spacing)
    cpr_img, N_frame, B_frame, positions, arclengths, ph, pw = _compute_cpr_data(
        vol.data.astype(np.float32),
        centerline_ijk,
        list(vol.spacing),
        width_mm=req.width_mm,
        pixels_wide=req.pixels_wide,
        pixels_high=req.pixels_high,
        rotation_deg=req.rotation_deg,
    )

    # Return CPR image as raw float32 + metadata as JSON header
    metadata = {
        "shape": [int(ph), int(pw)],
        "arclengths": arclengths.tolist(),
        "width_mm": req.width_mm,
    }
    # Pack: 4-byte metadata length + JSON metadata + raw float32 image
    meta_bytes = json.dumps(metadata).encode()
    meta_len = len(meta_bytes).to_bytes(4, 'little')
    image_bytes = cpr_img.astype(np.float32).tobytes()

    return Response(
        content=meta_len + meta_bytes + image_bytes,
        media_type="application/octet-stream",
    )

@router.post("/cross_section")
async def cross_section(req: CrossSectionRequest):
    """Compute a single cross-section at a given centerline position."""
    from visualize import _compute_cpr_data
    import scipy.ndimage as ndi

    vol = volumes.get(req.volume_id)
    if not vol:
        raise HTTPException(404, "Volume not found")

    centerline_mm = np.array(req.centerline_mm)
    spacing = np.array(vol.spacing)
    idx = min(max(0, req.position_idx), len(centerline_mm) - 1)

    # Get position and frame vectors at this point
    # Compute tangent, normal, binormal via finite differences
    if idx == 0:
        tangent = centerline_mm[1] - centerline_mm[0]
    elif idx >= len(centerline_mm) - 1:
        tangent = centerline_mm[-1] - centerline_mm[-2]
    else:
        tangent = centerline_mm[idx + 1] - centerline_mm[idx - 1]
    tangent = tangent / (np.linalg.norm(tangent) + 1e-12)

    # Build orthogonal frame
    up = np.array([0, 0, 1.0])
    if abs(np.dot(tangent, up)) > 0.9:
        up = np.array([0, 1.0, 0])
    normal = np.cross(tangent, up)
    normal /= np.linalg.norm(normal) + 1e-12
    binormal = np.cross(tangent, normal)

    # Apply rotation
    if req.rotation_deg != 0:
        angle = np.radians(req.rotation_deg)
        c, s = np.cos(angle), np.sin(angle)
        normal, binormal = c * normal + s * binormal, -s * normal + c * binormal

    # Sample cross-section image
    center = centerline_mm[idx]
    hw = req.width_mm
    px = req.pixels
    xs_img = np.zeros((px, px), dtype=np.float32)

    for row in range(px):
        for col in range(px):
            offset_n = (col / (px - 1) - 0.5) * 2 * hw
            offset_b = (row / (px - 1) - 0.5) * 2 * hw
            world_pt = center + offset_n * normal + offset_b * binormal
            voxel_pt = world_pt / spacing
            # Bounds check
            if all(0 <= voxel_pt[d] < vol.data.shape[d] for d in range(3)):
                xs_img[row, col] = ndi.map_coordinates(
                    vol.data, voxel_pt.reshape(3, 1), order=1
                )[0]

    metadata = json.dumps({"shape": [px, px], "arc_mm": float(np.sum(np.linalg.norm(np.diff(centerline_mm[:idx+1], axis=0), axis=1))) if idx > 0 else 0.0}).encode()
    meta_len = len(metadata).to_bytes(4, 'little')
    return Response(
        content=meta_len + metadata + xs_img.tobytes(),
        media_type="application/octet-stream",
    )
```

- [ ] **Step 2: Wire CPR routes into `main.py`**

```python
from cpr_routes import router as cpr_router
app.include_router(cpr_router)
```

- [ ] **Step 3: Create `CprView.svelte`**

Compound panel: straightened CPR (Canvas) on left 70%, 3 cross-sections stacked on right 30%. Draggable needle lines A/B/C on the CPR canvas. Fetches CPR from Python when centerline changes.

The implementer should:
1. Use HTML Canvas to render the CPR float32 image with W/L windowing
2. Draw 3 vertical needle lines (A=yellow, B=cyan, C=yellow) that are draggable
3. Show arc-length ticks at 10mm intervals
4. On needle drag → fetch cross-sections at those positions
5. Add rotation slider

- [ ] **Step 4: Create `CrossSection.svelte`**

Single cross-section panel. Renders the float32 image on Canvas with W/L. Shows title with arc-length position.

- [ ] **Step 5: Update `ContextPanel.svelte`**

When seeds have ≥2 points for any vessel, show `<CprView>`. Pass centerline data from seedStore.

- [ ] **Step 6: Test**

```
cargo tauri dev → Load DICOM → Place 2+ seeds → CPR appears in bottom-right → Drag needles → Cross-sections update
```

- [ ] **Step 7: Commit**

```bash
git add src/ pipeline_server/
git commit -m "feat: Phase 2b — CPR compound view with live cross-sections"
```

---

## Chunk 3: Phase 3 — Run Pipeline + FAI Dashboard

### Task 3.1: Pipeline Execution Endpoint

**Files:**
- Create: `pipeline_server/pipeline_routes.py`
- Modify: `pipeline_server/main.py`

- [ ] **Step 1: Create `pipeline_routes.py`**

SSE-streaming endpoint that runs the full pipeline per vessel: clip centerline → estimate radii → extract contours → build VOI → compute PCAT stats.

```python
# pipeline_server/pipeline_routes.py
import sys, os, json, asyncio
import numpy as np
from fastapi import APIRouter, HTTPException
from fastapi.responses import StreamingResponse
from pydantic import BaseModel
from typing import Dict, List, Optional

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "..", "pipeline"))

from volume_manager import volumes

router = APIRouter()

class VesselSeeds(BaseModel):
    ostium_mm: List[float]          # [z, y, x] in mm
    waypoints_mm: List[List[float]] # [[z,y,x], ...] in mm
    segment_start_mm: float = 0.0
    segment_length_mm: float = 40.0

class PipelineRequest(BaseModel):
    volume_id: str
    vessels: Dict[str, VesselSeeds]  # "LAD", "LCx", "RCA"

@router.post("/run_pipeline")
async def run_pipeline(req: PipelineRequest):
    """Run full PCAT pipeline with SSE progress streaming."""
    vol = volumes.get(req.volume_id)
    if not vol:
        raise HTTPException(404, "Volume not found")

    async def stream():
        from centerline import clip_centerline_by_arclength
        from pcat_segment import build_tubular_voi, compute_pcat_stats
        from contour_extraction import extract_vessel_contours, build_contour_based_voi

        volume = vol.data.astype(np.float32)
        spacing = list(vol.spacing)
        all_results = {}

        for vessel_name, seeds in req.vessels.items():
            try:
                yield f"data: {json.dumps({'stage': 'centerline', 'vessel': vessel_name, 'progress': 0.0})}\n\n"
                await asyncio.sleep(0)

                # Build centerline from seeds (spline)
                from scipy.interpolate import CubicSpline
                all_pts = [seeds.ostium_mm] + seeds.waypoints_mm
                pts = np.array(all_pts, dtype=np.float64)
                seg = np.linalg.norm(np.diff(pts, axis=0), axis=1)
                arc = np.concatenate([[0.0], np.cumsum(seg)])
                cs = CubicSpline(arc, pts, bc_type='natural')
                s_vals = np.linspace(0, arc[-1], int(np.ceil(arc[-1] / 0.5)))
                centerline_mm = cs(s_vals)
                centerline_ijk = centerline_mm / np.array(spacing)

                # Clip to proximal segment
                centerline_clipped = clip_centerline_by_arclength(
                    centerline_ijk, spacing,
                    start_mm=seeds.segment_start_mm,
                    length_mm=seeds.segment_length_mm,
                )

                yield f"data: {json.dumps({'stage': 'contour_extraction', 'vessel': vessel_name, 'progress': 0.2})}\n\n"
                await asyncio.sleep(0)

                # Extract contours
                contour_result = extract_vessel_contours(
                    volume, centerline_clipped, spacing, vessel_name
                )

                yield f"data: {json.dumps({'stage': 'voi_construction', 'vessel': vessel_name, 'progress': 0.6})}\n\n"
                await asyncio.sleep(0)

                # Build VOI
                voi_mask = build_contour_based_voi(
                    volume.shape,
                    contour_result.contours,
                    contour_result.positions_mm,
                    contour_result.N_frame,
                    contour_result.B_frame,
                    contour_result.r_eq,
                    spacing,
                )

                yield f"data: {json.dumps({'stage': 'fai_analysis', 'vessel': vessel_name, 'progress': 0.8})}\n\n"
                await asyncio.sleep(0)

                # Compute stats
                stats = compute_pcat_stats(volume, voi_mask, vessel_name)

                # Histogram data
                voi_hu = volume[voi_mask]
                hist_counts, hist_bins = np.histogram(voi_hu, bins=100, range=(-200, 200))

                all_results[vessel_name] = {
                    **stats,
                    "histogram": {"bins": hist_bins.tolist(), "counts": hist_counts.tolist()},
                }

                yield f"data: {json.dumps({'stage': 'complete', 'vessel': vessel_name, 'progress': 1.0})}\n\n"
                await asyncio.sleep(0)

            except Exception as e:
                yield f"data: {json.dumps({'stage': 'error', 'vessel': vessel_name, 'error': str(e)})}\n\n"

        yield f"data: {json.dumps({'status': 'complete', 'results': all_results})}\n\n"

    return StreamingResponse(stream(), media_type="text/event-stream")
```

- [ ] **Step 2: Wire into `main.py`**

- [ ] **Step 3: Test with curl**

```bash
curl -N -X POST http://127.0.0.1:PORT/run_pipeline \
  -H 'Content-Type: application/json' \
  -d '{"volume_id":"abc","vessels":{"LAD":{"ostium_mm":[100,200,150],"waypoints_mm":[[110,210,160],[120,220,170]],"segment_length_mm":40}}}'
```

- [ ] **Step 4: Commit**

```bash
git add pipeline_server/
git commit -m "feat(sidecar): pipeline execution endpoint with SSE progress streaming"
```

---

### Task 3.2: Pipeline Store + Progress UI

**Files:**
- Create: `src/lib/stores/pipelineStore.ts`
- Create: `src/components/ProgressOverlay.svelte`

- [ ] **Step 1: Create `pipelineStore.ts`**

Manages pipeline state: idle → running → complete. Stores results. Listens to SSE events.

```typescript
// src/lib/stores/pipelineStore.ts
import { sidecarFetch } from '../api';
import { seedStore } from './seedStore';

export type PipelineStatus = 'idle' | 'running' | 'complete' | 'error';
export type VesselProgress = { stage: string; progress: number };
export type VesselResult = {
  vessel: string;
  hu_mean: number;
  fai_risk: string;
  fat_fraction: number;
  n_voi_voxels: number;
  histogram: { bins: number[]; counts: number[] };
  [key: string]: unknown;
};

let status: PipelineStatus = $state('idle');
let progress: Record<string, VesselProgress> = $state({});
let results: Record<string, VesselResult> = $state({});
let errorMessage: string = $state('');

export const pipelineStore = {
  get status() { return status; },
  get progress() { return progress; },
  get results() { return results; },
  get error() { return errorMessage; },

  async run(volumeId: string) {
    status = 'running';
    progress = {};
    results = {};
    errorMessage = '';

    // Build request from seedStore
    const vessels: Record<string, unknown> = {};
    for (const v of seedStore.vessels) {
      const data = seedStore.getData(v);
      if (data.seeds.length < 2) continue;
      vessels[v] = {
        ostium_mm: data.seeds[0].position,
        waypoints_mm: data.seeds.slice(1).map(s => s.position),
        segment_length_mm: v === 'RCA' ? 40 : 40,
      };
    }

    try {
      const resp = await sidecarFetch('/run_pipeline', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ volume_id: volumeId, vessels }),
      });

      const reader = resp.body!.getReader();
      const decoder = new TextDecoder();
      let buffer = '';

      while (true) {
        const { done, value } = await reader.read();
        if (done) break;
        buffer += decoder.decode(value, { stream: true });

        const lines = buffer.split('\n');
        buffer = lines.pop() || '';

        for (const line of lines) {
          if (!line.startsWith('data: ')) continue;
          const data = JSON.parse(line.slice(6));

          if (data.status === 'complete') {
            results = data.results;
            status = 'complete';
          } else if (data.stage === 'error') {
            errorMessage = data.error;
            status = 'error';
          } else if (data.vessel) {
            progress = { ...progress, [data.vessel]: { stage: data.stage, progress: data.progress } };
          }
        }
      }
    } catch (e) {
      errorMessage = String(e);
      status = 'error';
    }
  },

  reset() {
    status = 'idle';
    progress = {};
    results = {};
    errorMessage = '';
  },
};
```

- [ ] **Step 2: Create `ProgressOverlay.svelte`**

Shows per-vessel progress bars during pipeline execution. Dismisses when complete.

- [ ] **Step 3: Commit**

```bash
git add src/lib/ src/components/
git commit -m "feat: pipeline store with SSE progress streaming + progress overlay"
```

---

### Task 3.3: FAI Dashboard

**Files:**
- Create: `src/components/AnalysisDashboard.svelte`
- Modify: `src/components/ContextPanel.svelte`
- Modify: `package.json` — add plotly.js

- [ ] **Step 1: Install Plotly.js**

```bash
npm install plotly.js-dist-min
```

- [ ] **Step 2: Create `AnalysisDashboard.svelte`**

Tabbed panel with 4 tabs:

**Tab 1: Overview** — Per-vessel cards showing FAI mean HU, risk badge (HIGH=red/LOW=green), fat fraction, voxel count.

**Tab 2: Histograms** — Plotly.js bar chart of HU distribution per vessel. Vertical lines at -190 (FAI min), -30 (FAI max), -70.1 (risk threshold).

**Tab 3: Radial Profile** — Plotly.js line chart of distance from vessel wall vs mean HU (if radial profile data available from pipeline).

**Tab 4: CPR + FAI** — The same CprView from Phase 2 but with FAI overlay (voxels in FAI range colored yellow→red on the cross-sections).

The implementer should use Plotly.js `newPlot()` in `onMount` with reactive updates when results change.

- [ ] **Step 3: Update `ContextPanel.svelte`**

Add case for `phase === 'analysis'` that renders `<AnalysisDashboard>` with results from pipelineStore.

- [ ] **Step 4: Add "Run Pipeline" button to toolbar**

In `App.svelte`, add a "Run Pipeline" button that calls `pipelineStore.run(volumeId)`. Button is disabled when no seeds are placed or pipeline is running. Show progress overlay while running.

- [ ] **Step 5: Test end-to-end**

```
cargo tauri dev → Load DICOM → Place seeds → Click "Run Pipeline" → See progress → FAI dashboard shows results
```

- [ ] **Step 6: Commit**

```bash
git add src/ package.json package-lock.json
git commit -m "feat: Phase 3 — FAI analysis dashboard with histograms and pipeline execution"
```

---

## Chunk 4: Phase 4 — Polish + Distribution

### Task 4.1: Toolbar + Settings

**Files:**
- Create: `src/components/SettingsDialog.svelte`
- Modify: `src/App.svelte` — enhanced toolbar

- [ ] **Step 1: Enhanced toolbar**

Add to the toolbar:
- Vessel selector (from SeedToolbar, already exists)
- W/L presets dropdown: Soft Tissue (W:400/L:40), Lung (W:1500/L:-600), Bone (W:2000/L:500), Custom
- "Run Pipeline" button with status indicator (idle/running/done)
- Export dropdown (PDF, .raw, DICOM)
- Settings gear icon

- [ ] **Step 2: Create `SettingsDialog.svelte`**

Modal dialog with:
- Segment length (default 40mm, number input)
- VOI mode (crisp / scaled radio buttons)
- Shell thickness: crisp_gap_mm (default 1.0), crisp_ring_mm (default 3.0)
- HU range: min (default -190), max (default -30)
- CPR slab thickness (default 3.0mm)
- Save to Svelte store; persisted via Tauri store plugin

- [ ] **Step 3: Commit**

```bash
git add src/
git commit -m "feat: toolbar enhancements + settings dialog"
```

---

### Task 4.2: Session Save/Load

**Files:**
- Modify: `src-tauri/src/lib.rs` — add save/load session commands
- Modify: `src-tauri/src/state.rs` — session serialization

- [ ] **Step 1: Add Rust commands for session persistence**

`save_session`: Serializes current state (seeds, settings, results) to JSON file. Uses native save dialog.

`load_session`: Reads JSON file, restores state. Uses native open dialog.

Session JSON format:
```json
{
  "version": 1,
  "dicom_path": "/path/to/dicom",
  "seeds": { "LAD": {...}, "LCx": {...}, "RCA": {...} },
  "settings": { "segment_length_mm": 40, ... },
  "results": { ... }
}
```

- [ ] **Step 2: Wire save/load into frontend**

Add "Save Session" / "Load Session" to toolbar dropdown. On load, restore seeds into seedStore and re-trigger DICOM loading.

- [ ] **Step 3: Commit**

```bash
git add src/ src-tauri/
git commit -m "feat: session save/load with JSON persistence"
```

---

### Task 4.3: Export (PDF, .raw, DICOM)

**Files:**
- Create: `pipeline_server/export_routes.py`
- Modify: `pipeline_server/main.py`

- [ ] **Step 1: Create export endpoints**

```
POST /export/pdf {volume_id, results, output_path}
  → Python: generates PDF report using existing pdf_report.py
  → Returns path to generated PDF

POST /export/raw {volume_id, vessel, voi_mask_id, output_path}
  → Python: exports .raw + metadata JSON using existing export_raw.py

POST /export/dicom {volume_id, cpr_data, output_path}
  → Python: DICOM secondary capture
```

- [ ] **Step 2: Wire export buttons in frontend**

Export dropdown calls appropriate Python endpoint, shows save dialog for output path.

- [ ] **Step 3: Commit**

```bash
git add pipeline_server/ src/
git commit -m "feat: export endpoints (PDF, .raw, DICOM secondary capture)"
```

---

### Task 4.4: Distribution Build

**Files:**
- Modify: `src-tauri/tauri.conf.json`
- Create: `pipeline_server/build_sidecar.py` (PyInstaller build script)

- [ ] **Step 1: Create PyInstaller build for sidecar**

```python
# pipeline_server/build_sidecar.py
# PyInstaller spec to bundle pipeline_server + pipeline dependencies
# Produces: dist/pipeline-server (single binary)
```

Build command: `pyinstaller --onefile --name pipeline-server pipeline_server/main.py --add-data "../pipeline:pipeline"`

- [ ] **Step 2: Update `sidecar.rs` for bundled mode**

When running in release mode (not dev), look for `pipeline-server` binary next to the app binary instead of `python3 pipeline_server/main.py`.

- [ ] **Step 3: Configure Tauri bundle**

Update `tauri.conf.json` to include the PyInstaller binary as an external resource. Configure `.dmg` settings (icon, background, etc.).

- [ ] **Step 4: Build and test**

```bash
npm run build           # Build Svelte frontend
tauri build             # Build .dmg
# Install on clean Mac, verify full workflow
```

- [ ] **Step 5: Commit**

```bash
git add .
git commit -m "feat: Phase 4 — distribution build with PyInstaller sidecar"
```

---

## Verification Checklist

After all phases:

- [ ] Phase 1: Load real patient DICOM → 3 synced MPR views
- [ ] Phase 2: Place seeds → live centerline + CPR with 3 cross-sections
- [ ] Phase 3: Run Pipeline → FAI results match current Python app output
- [ ] Phase 4: .dmg installs and runs full workflow on clean Mac
