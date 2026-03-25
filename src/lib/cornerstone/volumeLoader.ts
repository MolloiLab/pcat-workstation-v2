/**
 * Volume loader — fetches all axial slices from the Rust backend via Tauri IPC
 * and creates a cornerstone3D local volume.
 *
 * Rust metadata uses [Z, Y, X] ordering (ndarray convention).
 * cornerstone3D `createLocalVolume` uses [X, Y, Z] (columns, rows, slices).
 * This loader handles the axis swap.
 */
import { volumeLoader, cache, type Types } from '@cornerstonejs/core';

type Point3 = Types.Point3;
type Mat3 = Types.Mat3;

import { getSlice } from '$lib/api';
import type { VolumeMetadata } from '$lib/stores/volumeStore.svelte';

/** Concurrency limit for parallel slice fetches via Tauri IPC. */
const FETCH_CONCURRENCY = 8;

/**
 * Fetch all axial slices from the Rust backend and pack into a cornerstone3D
 * local volume.
 *
 * @param meta       - Volume metadata returned by `load_dicom` command.
 * @param onProgress - Optional callback invoked with progress 0-100.
 * @returns The cornerstone volume ID.
 */
export async function loadVolume(
  meta: VolumeMetadata,
  onProgress?: (percent: number) => void,
): Promise<string> {
  const csVolumeId = `pcat:${meta.volumeId}`;

  // If the volume is already cached, return immediately.
  const existing = cache.getVolume(csVolumeId);
  if (existing) {
    onProgress?.(100);
    return csVolumeId;
  }

  // --- Axis swap: Rust [Z, Y, X] -> cornerstone [X, Y, Z] ---
  const [numSlices, numRows, numCols] = meta.shape;
  const [sz, sy, sx] = meta.spacing;

  const dimensions: Point3 = [numCols, numRows, numSlices];
  const spacing: Point3 = [sx, sy, sz];
  const origin: Point3 = [meta.origin[2], meta.origin[1], meta.origin[0]];
  // Direction matrix from Rust DICOM loader is already in DICOM patient coords
  // (ImageOrientationPatient row/col + cross product for Z).
  // cornerstone3D also uses patient coordinate system — no swap needed.
  const direction = (meta.direction.length === 9
    ? meta.direction
    : [1, 0, 0, 0, 1, 0, 0, 0, 1]) as Mat3;

  // --- Allocate the full scalar data buffer ---
  const sliceLength = numCols * numRows;
  const totalVoxels = sliceLength * numSlices;
  const scalarData = new Int16Array(totalVoxels);

  // --- Fetch slices via Tauri IPC with bounded concurrency ---
  let completedSlices = 0;

  const fetchSlice = async (idx: number): Promise<void> => {
    const arrayBuf = await getSlice('axial', idx);
    const sliceData = new Int16Array(arrayBuf);
    scalarData.set(sliceData, idx * sliceLength);

    completedSlices++;
    onProgress?.(Math.round((completedSlices / numSlices) * 100));
  };

  await runWithConcurrency(
    Array.from({ length: numSlices }, (_, i) => () => fetchSlice(i)),
    FETCH_CONCURRENCY,
  );

  // --- Build cornerstone volume metadata ---
  const csMetadata = {
    BitsAllocated: 16,
    BitsStored: 16,
    SamplesPerPixel: 1,
    HighBit: 15,
    PhotometricInterpretation: 'MONOCHROME2',
    PixelRepresentation: 1,
    Modality: 'CT',
    ImageOrientationPatient: Array.from(direction.slice(0, 6)),
    PixelSpacing: [sy, sx],
    FrameOfReferenceUID: `1.2.826.0.1.3680043.8.498.pcat`,
    Columns: numCols,
    Rows: numRows,
    voiLut: [
      { windowCenter: meta.windowCenter, windowWidth: meta.windowWidth },
    ],
    VOILUTFunction: 'LINEAR',
  };

  // --- Create the local volume and cache it ---
  volumeLoader.createLocalVolume(csVolumeId, {
    scalarData,
    metadata: csMetadata,
    dimensions,
    spacing,
    origin,
    direction,
  });

  return csVolumeId;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/**
 * Swap the 3x3 direction matrix from [Z,Y,X]-relative to [X,Y,Z]-relative.
 */
function swapDirectionMatrix(dir: number[]): number[] {
  if (dir.length !== 9) return [1, 0, 0, 0, 1, 0, 0, 0, 1];
  return [
    dir[2], dir[1], dir[0],
    dir[5], dir[4], dir[3],
    dir[8], dir[7], dir[6],
  ];
}

/**
 * Execute an array of async task factories with bounded concurrency.
 */
async function runWithConcurrency(
  tasks: (() => Promise<void>)[],
  concurrency: number,
): Promise<void> {
  let idx = 0;
  const worker = async () => {
    while (idx < tasks.length) {
      const taskIdx = idx++;
      await tasks[taskIdx]();
    }
  };
  const workers = Array.from(
    { length: Math.min(concurrency, tasks.length) },
    () => worker(),
  );
  await Promise.all(workers);
}
