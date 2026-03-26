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

/** Save seeds JSON to app data directory. Returns the file path. */
export async function saveSeeds(seedsJson: string): Promise<string> {
  return invoke<string>('save_seeds', { seedsJson });
}

/** Load seeds JSON from app data directory. Returns null if no file. */
export async function loadSeeds(): Promise<string | null> {
  return invoke<string | null>('load_seeds');
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
