/**
 * Reactive store for per-vessel seed placement and centerline computation.
 *
 * Uses Svelte 5 runes (`$state`) for fine-grained reactivity.
 * Centerlines are recomputed immediately whenever seeds change.
 */

import { computeSplineCenterline } from '$lib/spline';

export type Vessel = 'LAD' | 'LCx' | 'RCA';

export type Seed = {
  position: [number, number, number];
};

export type VesselData = {
  seeds: Seed[];
  centerline: [number, number, number][] | null;
  /** Fractional position (0..1) along the centerline marking the ostium. */
  ostiumFraction: number | null;
};

/** Vessel display colors. */
export const VESSEL_COLORS: Record<Vessel, string> = {
  LAD: '#ff8c00',
  LCx: '#4488ff',
  RCA: '#44cc44',
};

const ALL_VESSELS: Vessel[] = ['RCA', 'LAD', 'LCx'];

/** Create a fresh empty vessel data object. */
function emptyVesselData(): VesselData {
  return { seeds: [], centerline: null, ostiumFraction: null };
}

/**
 * Recompute the centerline from seeds (preserving ostiumFraction).
 */
function recomputeCenterline(data: VesselData): VesselData {
  if (data.seeds.length < 2) {
    return { ...data, centerline: null };
  }
  const points = data.seeds.map((s) => s.position);
  const centerline = computeSplineCenterline(points);
  return { ...data, centerline };
}

// --- Reactive state ---

let activeVessel: Vessel = $state('RCA');

let vesselData: Record<Vessel, VesselData> = $state({
  LAD: emptyVesselData(),
  LCx: emptyVesselData(),
  RCA: emptyVesselData(),
});

/** Index of the selected seed within the active vessel (null = no selection). */
let selectedSeedIndex: number | null = $state(null);

export const seedStore = {
  // --- Getters ---

  get activeVessel(): Vessel {
    return activeVessel;
  },
  get vessels(): Record<Vessel, VesselData> {
    return vesselData;
  },
  /** Convenience: data for the currently active vessel. */
  get activeVesselData(): VesselData {
    return vesselData[activeVessel];
  },
  /** Index of the selected seed in the active vessel, or null. */
  get selectedSeedIndex(): number | null {
    return selectedSeedIndex;
  },

  // --- Vessel selection ---

  setActiveVessel(vessel: Vessel) {
    activeVessel = vessel;
    // Deselect when switching vessels
    selectedSeedIndex = null;
  },

  // --- Seed selection ---

  /** Select a seed by index within the active vessel. */
  selectSeed(index: number) {
    const current = vesselData[activeVessel];
    if (index < 0 || index >= current.seeds.length) return;
    selectedSeedIndex = index;
  },

  /** Clear the current seed selection. */
  deselectSeed() {
    selectedSeedIndex = null;
  },

  // --- Seed manipulation (operates on active vessel) ---

  /**
   * Add a seed at the given world-space position.
   * The first seed becomes 'ostium'; subsequent seeds are 'waypoint'.
   */
  addSeed(position: [number, number, number]) {
    const v = activeVessel;
    const current = vesselData[v];
    const newSeeds = [...current.seeds, { position }];
    vesselData[v] = recomputeCenterline({ ...current, seeds: newSeeds });
    // Don't auto-select — it causes cascading effects that freeze the UI.
    // Navigation only happens when user explicitly clicks an existing seed.
    selectedSeedIndex = null;
  },

  /**
   * Insert a seed at a specific index within the active vessel.
   * Used for click-on-centerline insertion between existing seeds.
   */
  insertSeedAt(index: number, position: [number, number, number]) {
    const v = activeVessel;
    const current = vesselData[v];
    if (index < 0 || index > current.seeds.length) return;
    const newSeeds = [...current.seeds];
    newSeeds.splice(index, 0, { position });
    vesselData[v] = recomputeCenterline({ ...current, seeds: newSeeds });
    // Select the newly inserted seed
    selectedSeedIndex = index;
  },

  /**
   * Remove the seed at the given index from the active vessel.
   */
  removeSeed(index: number) {
    const v = activeVessel;
    const current = vesselData[v];
    if (index < 0 || index >= current.seeds.length) return;
    const newSeeds = current.seeds.filter((_, i) => i !== index);
    vesselData[v] = recomputeCenterline({ ...current, seeds: newSeeds });
    // Clear selection if the removed seed was selected, or adjust index
    if (selectedSeedIndex !== null) {
      if (selectedSeedIndex === index) {
        selectedSeedIndex = null;
      } else if (selectedSeedIndex > index) {
        selectedSeedIndex--;
      }
    }
  },

  /**
   * Move an existing seed to a new position.
   */
  moveSeed(index: number, position: [number, number, number]) {
    const v = activeVessel;
    const current = vesselData[v];
    if (index < 0 || index >= current.seeds.length) return;
    const newSeeds = current.seeds.map((s, i) =>
      i === index ? { ...s, position } : s,
    );
    vesselData[v] = recomputeCenterline({ ...current, seeds: newSeeds });
  },

  // --- Ostium fraction ---

  /** Set the ostium marker at a fractional position along the active vessel's centerline. */
  setOstiumFraction(fraction: number | null) {
    const v = activeVessel;
    vesselData[v] = { ...vesselData[v], ostiumFraction: fraction };
  },

  /** Get the ostium fraction for a vessel. */
  getOstiumFraction(vessel: Vessel): number | null {
    return vesselData[vessel].ostiumFraction;
  },

  /**
   * Get the interpolated 3D world position at the ostium for a vessel.
   * Returns null if no ostiumFraction is set or no centerline exists.
   */
  getOstiumWorldPosForVessel(vessel: Vessel): [number, number, number] | null {
    const data = vesselData[vessel];
    if (data.ostiumFraction === null || !data.centerline || data.centerline.length < 2) {
      return null;
    }
    const cl = data.centerline;
    const fIdx = data.ostiumFraction * (cl.length - 1);
    const i0 = Math.floor(fIdx);
    const i1 = Math.min(i0 + 1, cl.length - 1);
    const t = fIdx - i0;
    return [
      cl[i0][0] + t * (cl[i1][0] - cl[i0][0]),
      cl[i0][1] + t * (cl[i1][1] - cl[i0][1]),
      cl[i0][2] + t * (cl[i1][2] - cl[i0][2]),
    ];
  },

  // --- Bulk operations ---

  /** Clear all seeds and centerline for a specific vessel. */
  clearVessel(vessel: Vessel) {
    vesselData[vessel] = emptyVesselData();
    selectedSeedIndex = null;
  },

  /** Clear all seeds and centerlines for every vessel. */
  clearAll() {
    for (const v of ALL_VESSELS) {
      vesselData[v] = emptyVesselData();
    }
    selectedSeedIndex = null;
  },

  // --- Export / Import ---

  /** Export all seed data as a JSON string. */
  exportJson(): string {
    const data: Record<string, { seeds: [number, number, number][]; ostiumFraction: number | null }> = {};
    for (const v of ALL_VESSELS) {
      const vd = vesselData[v];
      data[v] = {
        seeds: vd.seeds.map((s) => s.position),
        ostiumFraction: vd.ostiumFraction,
      };
    }
    return JSON.stringify({ activeVessel, vessels: data }, null, 2);
  },

  /** Import seed data from a JSON string (exported by exportJson). */
  importJson(json: string) {
    const parsed = JSON.parse(json);
    if (parsed.activeVessel) {
      activeVessel = parsed.activeVessel;
    }
    if (parsed.vessels) {
      for (const v of ALL_VESSELS) {
        const vd = parsed.vessels[v];
        if (!vd) continue;
        const seeds = (vd.seeds ?? []).map((pos: [number, number, number]) => ({ position: pos }));
        vesselData[v] = recomputeCenterline({
          seeds,
          centerline: null,
          ostiumFraction: vd.ostiumFraction ?? null,
        });
      }
    }
    selectedSeedIndex = null;
  },
};
