/**
 * Reactive store for the currently loaded CT volume.
 *
 * Uses Svelte 5 runes (`$state`) for fine-grained reactivity.
 * Import `volumeStore` from any `.svelte` or `.svelte.ts` file.
 */

import { cache } from '@cornerstonejs/core';

export type VolumeMetadata = {
  volumeId: string;
  /** Dimensions in Python/NumPy order: [Z, Y, X] (slices, rows, columns). */
  shape: [number, number, number];
  /** Voxel spacing in Python order: [sz, sy, sx]. */
  spacing: [number, number, number];
  origin: [number, number, number];
  /** Row-major 3x3 direction cosine matrix (9 elements). */
  direction: number[];
  windowCenter: number;
  windowWidth: number;
  patientName: string;
  studyDescription: string;
};

let currentVolume = $state<VolumeMetadata | null>(null);
let cornerstoneVolumeId = $state<string | null>(null);
let loading = $state(false);
let loadProgress = $state(0);

export const volumeStore = {
  get current() {
    return currentVolume;
  },
  get cornerstoneVolumeId() {
    return cornerstoneVolumeId;
  },
  get loading() {
    return loading;
  },
  /** Progress percentage 0-100. */
  get loadProgress() {
    return loadProgress;
  },

  set(vol: VolumeMetadata) {
    currentVolume = vol;
  },
  setCornerstoneVolumeId(id: string) {
    cornerstoneVolumeId = id;
  },
  setLoading(v: boolean) {
    loading = v;
  },
  setLoadProgress(v: number) {
    loadProgress = v;
  },
  clear() {
    // Purge old volume from cornerstone cache
    if (cornerstoneVolumeId) {
      try { cache.removeVolumeLoadObject(cornerstoneVolumeId); } catch { /* ignore */ }
    }
    currentVolume = null;
    cornerstoneVolumeId = null;
    loading = false;
    loadProgress = 0;
  },
};
