/**
 * CPR projection / unprojection utilities.
 *
 * Maps between 3-D world coordinates ([z, y, x] ordering, consistent with
 * the Rust backend) and 2-D canvas coordinates for both straightened and
 * curved CPR views.
 */

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export type CprProjectionInfo = {
  total_arc_mm: number;
  half_width_mm: number;
  view_right: [number, number, number];
  view_up: [number, number, number];
  view_center: [number, number, number];
  bbox_mm: [number, number, number, number]; // [min_x, max_x, min_y, max_y]
  positions: [number, number, number][];
  arclengths: number[];
  normals: [number, number, number][];
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Dot product of two 3-component vectors. */
function dot3(a: [number, number, number], b: [number, number, number]): number {
  return a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
}

/**
 * Linear interpolation between two 3-component vectors.
 * Returns `a * (1 - t) + b * t`.
 */
function lerp3(
  a: [number, number, number],
  b: [number, number, number],
  t: number,
): [number, number, number] {
  return [
    a[0] + (b[0] - a[0]) * t,
    a[1] + (b[1] - a[1]) * t,
    a[2] + (b[2] - a[2]) * t,
  ];
}

/** Subtract two 3-component vectors: `a - b`. */
function sub3(
  a: [number, number, number],
  b: [number, number, number],
): [number, number, number] {
  return [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
}

/** Squared Euclidean distance between two 3-component vectors. */
function distSq3(a: [number, number, number], b: [number, number, number]): number {
  const dz = a[0] - b[0];
  const dy = a[1] - b[1];
  const dx = a[2] - b[2];
  return dz * dz + dy * dy + dx * dx;
}

// Small pixel margin for bounds checks.
const MARGIN_PX = 2;

// ---------------------------------------------------------------------------
// 1. World -> Straightened CPR
// ---------------------------------------------------------------------------

/**
 * Map a 3-D world position to straightened CPR canvas coordinates.
 *
 * Returns `[canvasX, canvasY]` or `null` when the projected point falls
 * outside the canvas (with a small margin).
 */
export function worldToStraightenedCpr(
  worldZyx: [number, number, number],
  info: CprProjectionInfo,
  canvasW: number,
  canvasH: number,
): [number, number] | null {
  const { positions, normals, half_width_mm } = info;
  const n = positions.length;
  if (n === 0) return null;

  // 1. Find closest centerline point.
  let bestIdx = 0;
  let bestDist = distSq3(worldZyx, positions[0]);
  for (let i = 1; i < n; i++) {
    const d = distSq3(worldZyx, positions[i]);
    if (d < bestDist) {
      bestDist = d;
      bestIdx = i;
    }
  }

  // 2. Column index -> canvasX.
  const canvasX = (bestIdx / (n - 1)) * canvasW;

  // 3. Lateral offset = projection of (world - centerline) onto the normal.
  const offset = sub3(worldZyx, positions[bestIdx]);
  const lateralOffset = dot3(offset, normals[bestIdx]);

  // 4. Map lateral offset -> canvasY.
  const canvasY = (0.5 - lateralOffset / (2 * half_width_mm)) * canvasH;

  // 5. Bounds check with margin.
  if (
    canvasX < -MARGIN_PX ||
    canvasX > canvasW + MARGIN_PX ||
    canvasY < -MARGIN_PX ||
    canvasY > canvasH + MARGIN_PX
  ) {
    return null;
  }

  return [canvasX, canvasY];
}

// ---------------------------------------------------------------------------
// 2. World -> Curved CPR
// ---------------------------------------------------------------------------

/**
 * Map a 3-D world position to curved CPR canvas coordinates.
 *
 * Returns `[canvasX, canvasY]` or `null` when the projected point falls
 * outside the canvas.
 */
export function worldToCurvedCpr(
  worldZyx: [number, number, number],
  info: CprProjectionInfo,
  canvasW: number,
  canvasH: number,
): [number, number] | null {
  const { view_center, view_right, view_up, bbox_mm } = info;

  // 1. Offset from view center.
  const offset = sub3(worldZyx, view_center);

  // 2. Project onto viewing plane.
  const x_mm = dot3(offset, view_right);
  const y_mm = dot3(offset, view_up);

  // 3. Map mm -> canvas pixels via bbox.
  const bboxW = bbox_mm[1] - bbox_mm[0];
  const bboxH = bbox_mm[3] - bbox_mm[2];
  if (bboxW === 0 || bboxH === 0) return null;

  const canvasX = ((x_mm - bbox_mm[0]) / bboxW) * (canvasW - 1);
  const canvasY = ((bbox_mm[3] - y_mm) / bboxH) * (canvasH - 1);

  // 4. Bounds check.
  if (
    canvasX < -MARGIN_PX ||
    canvasX > canvasW - 1 + MARGIN_PX ||
    canvasY < -MARGIN_PX ||
    canvasY > canvasH - 1 + MARGIN_PX
  ) {
    return null;
  }

  return [canvasX, canvasY];
}

// ---------------------------------------------------------------------------
// 3. Straightened CPR -> World
// ---------------------------------------------------------------------------

/**
 * Map a straightened CPR canvas position back to 3-D world coordinates.
 *
 * Uses linear interpolation between centerline positions and normals for
 * fractional column indices.
 */
export function straightenedCprToWorld(
  canvasX: number,
  canvasY: number,
  info: CprProjectionInfo,
  canvasW: number,
  canvasH: number,
): [number, number, number] {
  const { positions, normals, half_width_mm } = info;
  const n = positions.length;

  // 1. Canvas X -> fractional index.
  const frac = canvasX / canvasW;
  const floatIdx = frac * (n - 1);
  const idx0 = Math.max(0, Math.min(n - 2, Math.floor(floatIdx)));
  const t = floatIdx - idx0;

  // 2. Interpolated centerline position.
  const pos = lerp3(positions[idx0], positions[idx0 + 1], t);

  // 3. Canvas Y -> lateral offset.
  const lateral = (0.5 - canvasY / canvasH) * 2 * half_width_mm;

  // 4. Interpolated normal.
  const normal = lerp3(normals[idx0], normals[idx0 + 1], t);

  // 5. Reconstruct world position = pos + lateral * normal.
  return [
    pos[0] + lateral * normal[0],
    pos[1] + lateral * normal[1],
    pos[2] + lateral * normal[2],
  ];
}

// ---------------------------------------------------------------------------
// 4. Curved CPR -> World
// ---------------------------------------------------------------------------

/**
 * Map a curved CPR canvas position back to 3-D world coordinates.
 *
 * The reconstruction lies on the viewing plane defined by `view_center`,
 * `view_right`, and `view_up`.
 */
export function curvedCprToWorld(
  canvasX: number,
  canvasY: number,
  info: CprProjectionInfo,
  canvasW: number,
  canvasH: number,
): [number, number, number] {
  const { view_center, view_right, view_up, bbox_mm } = info;

  // 1. Canvas pixels -> mm in viewing plane.
  const x_mm = bbox_mm[0] + (canvasX / (canvasW - 1)) * (bbox_mm[1] - bbox_mm[0]);
  const y_mm = bbox_mm[3] - (canvasY / (canvasH - 1)) * (bbox_mm[3] - bbox_mm[2]);

  // 2. Reconstruct 3-D position on the viewing plane.
  return [
    view_center[0] + x_mm * view_right[0] + y_mm * view_up[0],
    view_center[1] + x_mm * view_right[1] + y_mm * view_up[1],
    view_center[2] + x_mm * view_right[2] + y_mm * view_up[2],
  ];
}
