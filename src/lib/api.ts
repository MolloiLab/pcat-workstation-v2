/**
 * API layer — Tauri IPC commands to the Rust backend.
 * No HTTP, no Python sidecar. All calls go through `invoke()`.
 */
import { invoke } from '@tauri-apps/api/core';

export type VolumeInfo = {
  shape: [number, number, number]; // [Z, Y, X]
  spacing: [number, number, number]; // [sz, sy, sx]
  origin: [number, number, number];
  direction: number[];
  window_center: number;
  window_width: number;
  patient_name: string;
  study_description: string;
};

/** Open native folder picker. Returns path or null if cancelled. */
export async function openDicomDialog(): Promise<string | null> {
  return invoke<string | null>('open_dicom_dialog');
}

/** Load DICOM directory into Rust backend. Returns volume metadata. */
export async function loadDicom(path: string): Promise<VolumeInfo> {
  return invoke<VolumeInfo>('load_dicom', { path });
}

/** Get list of recently opened DICOM folder paths. */
export async function getRecentDicoms(): Promise<string[]> {
  return invoke<string[]>('get_recent_dicoms');
}

/** Save seeds JSON to app data directory, keyed by DICOM path. Returns the file path. */
export async function saveSeeds(seedsJson: string, dicomPath: string): Promise<string> {
  return invoke<string>('save_seeds', { seedsJson, dicomPath });
}

/** Load seeds JSON from app data directory, keyed by DICOM path. Returns null if no file. */
export async function loadSeeds(dicomPath: string): Promise<string | null> {
  return invoke<string | null>('load_seeds', { dicomPath });
}

/**
 * Get a single slice as raw i16 LE bytes from the Rust backend.
 * Tauri serializes Vec<u8> as a number[], so we convert back.
 */
export async function getSlice(
  axis: string,
  idx: number,
): Promise<ArrayBuffer> {
  const bytes = await invoke<number[]>('get_slice', { axis, idx });
  return new Uint8Array(bytes).buffer;
}

/* ── Dual-energy series scanning & loading ─────────────────── */

export type SeriesInfo = {
  series_uid: string;
  description: string;
  num_slices: number;
  kev_label: number | null;
};

export type DualEnergyInfo = {
  shape: [number, number, number];
  spacing: [number, number, number];
  low_kev: number;
  high_kev: number;
  patient_name: string;
};

/** Scan DICOM directory for available series. */
export async function scanSeries(path: string): Promise<SeriesInfo[]> {
  return invoke<SeriesInfo[]>('scan_series', { path });
}

/** Load dual-energy volumes from two selected series. */
export async function loadDualEnergy(
  path: string,
  lowSeriesUid: string,
  highSeriesUid: string,
  lowKev: number,
  highKev: number,
): Promise<DualEnergyInfo> {
  return invoke<DualEnergyInfo>('load_dual_energy', {
    path, lowSeriesUid, highSeriesUid, lowKev, highKev,
  });
}

/* ── Annotation + Snake commands ─────────────────────────── */

export type AnnotationTarget = {
  image: number[];         // HU values, row-major, pixels*pixels
  pixels: number;
  width_mm: number;
  arc_mm: number;
  frame_index: number;
  vessel_wall: [number, number][];  // [x,y] pixel coords
  vessel_radius_mm: number;
  init_boundary: [number, number][]; // [x,y] pixel coords
};

export type SnakeResult = {
  points: [number, number][];
  iterations: number;
  max_displacement: number;
  converged: boolean;
};

export type MmdSummary = {
  method: string;
  iterations: number;
  converged: boolean;
  n_voxels: number;
  mean_water_frac: number;
  mean_lipid_frac: number;
  mean_iodine_frac: number;
  mean_calcium_frac: number;
};

/** Generate annotation targets for all cross-section frames along a centerline. */
export async function generateAnnotationTargets(
  centerlineMm: [number, number, number][],
): Promise<AnnotationTarget[]> {
  return invoke<AnnotationTarget[]>('generate_annotation_targets', {
    centerlineMm,
  });
}

/** Initialize a circular snake contour for a given annotation target. */
export async function initSnake(
  targetIndex: number,
  initRadiusMm?: number,
): Promise<[number, number][]> {
  return invoke<[number, number][]>('init_snake', {
    targetIndex,
    initRadiusMm: initRadiusMm ?? null,
  });
}

/** Evolve the active contour (snake) for a target. */
export async function evolveSnake(
  targetIndex: number,
  nIterations: number = 200,
): Promise<SnakeResult> {
  return invoke<SnakeResult>('evolve_snake', {
    targetIndex,
    nIterations,
  });
}

/** Replace the snake control points for a target (after manual drag). */
export async function updateSnakePoints(
  targetIndex: number,
  points: [number, number][],
): Promise<void> {
  return invoke<void>('update_snake_points', {
    targetIndex,
    points,
  });
}

/** Insert a new control point on the snake contour at a given position. */
export async function addSnakePoint(
  targetIndex: number,
  position: [number, number],
): Promise<number> {
  return invoke<number>('add_snake_point', {
    targetIndex,
    position,
  });
}

/** Finalize the contour for a target (marks it as done). */
export async function finalizeContour(
  targetIndex: number,
): Promise<void> {
  return invoke<void>('finalize_contour', {
    targetIndex,
  });
}

/** Run multi-material decomposition on the annotated ROI. */
export async function runMmdOnRoi(
  method: string = 'direct',
): Promise<MmdSummary> {
  return invoke<MmdSummary>('run_mmd_on_roi', { method });
}

/* ── Surface sampling + MMD overlay ─────────────────────── */

export type CrossSectionSurface = {
  arc_mm: number;
  theta_deg: number[];
  r_mm: number[];
  surface: number[];
  n_theta: number;
  n_radial: number;
  max_r_per_theta: number[];
};

/** Sample radial-angular surface data from MMD result for all finalized cross-sections. */
export async function sampleSurfaces(
  material: string,
  unit: string,
): Promise<CrossSectionSurface[]> {
  return invoke<CrossSectionSurface[]>('sample_surfaces', { material, unit });
}

/** Get MMD material overlay for a single cross-section (flat f32 array). */
export async function getMmdOverlay(
  targetIndex: number,
  material: string,
  unit: string,
): Promise<number[]> {
  return invoke<number[]>('get_mmd_overlay', { targetIndex, material, unit });
}
