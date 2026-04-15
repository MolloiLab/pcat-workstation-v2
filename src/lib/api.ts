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
