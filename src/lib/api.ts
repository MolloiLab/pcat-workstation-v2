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

// ---------------------------------------------------------------------------
// MMD (Multi-Material Decomposition)
// ---------------------------------------------------------------------------

export type MonoVolumeInfo = {
  energies: number[];
  shape: [number, number, number];
  spacing: [number, number, number];
};

export type MmdSummary = {
  shape: [number, number, number];
  elapsed_ms: number;
  mean_water: number;
  mean_lipid: number;
  mean_iodine: number;
  mean_residual: number;
};

/** Load mono-energetic DICOM volumes. paths: keV label → directory path. */
export async function loadMonoVolumes(paths: Record<string, string>): Promise<MonoVolumeInfo> {
  return invoke<MonoVolumeInfo>('load_mono_volumes', { paths });
}

/** Run multi-material decomposition on loaded mono-energetic volumes. */
export async function runMmd(config: {
  basis_lacs: number[][];
  noise_variances: number[];
  hu_upper: number;
  hu_lower: number;
}): Promise<MmdSummary> {
  return invoke<MmdSummary>('run_mmd', { config });
}

/** Get a 2D slice from an MMD material fraction map. */
export async function getMmdSlice(
  material: string,
  axis: string,
  idx: number,
): Promise<ArrayBuffer> {
  const bytes = await invoke<number[]>('get_mmd_slice', { material, axis, idx });
  return new Float32Array(new Uint8Array(bytes).buffer).buffer;
}
