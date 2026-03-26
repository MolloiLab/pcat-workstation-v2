/**
 * Reactive store for per-vessel seed placement and centerline computation.
 *
 * Uses Svelte 5 runes (`$state`) for fine-grained reactivity.
 * Centerlines are recomputed immediately whenever seeds change.
 */

import { computeSplineCenterline } from '$lib/spline';

export type Vessel = 'LAD' | 'LCx' | 'RCA';
export type SeedType = 'ostium' | 'waypoint';

export type Seed = {
  position: [number, number, number];
  type: SeedType;
};

export type VesselData = {
  seeds: Seed[];
  centerline: [number, number, number][] | null;
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
  return { seeds: [], centerline: null };
}

/**
 * Recompute the centerline from seeds.
 * First seed is ostium, rest are waypoints — all positions feed the spline.
 */
function recomputeCenterline(data: VesselData): VesselData {
  if (data.seeds.length < 2) {
    return { seeds: data.seeds, centerline: null };
  }
  const points = data.seeds.map((s) => s.position);
  const centerline = computeSplineCenterline(points);
  return { seeds: data.seeds, centerline };
}

/**
 * Assign seed types: first seed is 'ostium', rest are 'waypoint'.
 */
function assignSeedTypes(seeds: Seed[]): Seed[] {
  return seeds.map((s, i) => ({
    ...s,
    type: i === 0 ? 'ostium' : 'waypoint',
  }));
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
    const newSeeds = assignSeedTypes([
      ...current.seeds,
      { position, type: 'waypoint' },
    ]);
    vesselData[v] = recomputeCenterline({ seeds: newSeeds, centerline: null });
    // Auto-select the newly placed seed so navigation triggers
    selectedSeedIndex = newSeeds.length - 1;
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
    newSeeds.splice(index, 0, { position, type: 'waypoint' });
    vesselData[v] = recomputeCenterline({
      seeds: assignSeedTypes(newSeeds),
      centerline: null,
    });
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
    const newSeeds = assignSeedTypes(
      current.seeds.filter((_, i) => i !== index),
    );
    vesselData[v] = recomputeCenterline({ seeds: newSeeds, centerline: null });
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
    vesselData[v] = recomputeCenterline({ seeds: newSeeds, centerline: null });
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
};
