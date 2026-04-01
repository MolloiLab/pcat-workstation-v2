/**
 * Reactive store for Multi-Material Decomposition (MMD) state.
 *
 * Tracks loaded mono-energetic volumes, decomposition config,
 * and result metadata.
 */
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

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

export type MmdConfig = {
  lipid_hu: [number, number, number, number];
  iodine_hu: [number, number, number, number];
  noise_variances: [number, number, number, number];
  hu_upper: number;
  hu_lower: number;
};

type MmdStatus = 'idle' | 'loading' | 'loaded' | 'running' | 'complete' | 'error';

function createMmdStore() {
  let status: MmdStatus = $state('idle');
  let monoInfo: MonoVolumeInfo | null = $state(null);
  let result: MmdSummary | null = $state(null);
  let progress = $state(0);
  let error = $state('');

  /** Paths for each mono-energy directory, keyed by keV label. */
  let monoPaths: Record<string, string> = $state({});

  /** Default config — user can override via UI.
   *  lipid_hu / iodine_hu: measure from ROIs in your own mono-energetic images.
   *  Defaults are rough starting points for cardiac CTA with iodine contrast. */
  let config: MmdConfig = $state({
    lipid_hu: [-95, -85, -78, -75],          // subcutaneous fat ROI at 70/100/140/150 keV
    iodine_hu: [300, 150, 60, 50],           // contrast-enhanced aorta ROI
    noise_variances: [100, 100, 100, 100],   // ~10 HU std per energy
    hu_upper: 500.0,
    hu_lower: -500.0,
  });

  return {
    get status() { return status; },
    get monoInfo() { return monoInfo; },
    get result() { return result; },
    get progress() { return progress; },
    get error() { return error; },
    get monoPaths() { return monoPaths; },
    get config() { return config; },

    setMonoPath(keV: string, path: string) {
      monoPaths = { ...monoPaths, [keV]: path };
    },

    setConfig(c: MmdConfig) {
      config = c;
    },

    async loadMonoVolumes() {
      if (Object.keys(monoPaths).length < 4) {
        error = 'Need 4 mono-energetic volume paths';
        status = 'error';
        return;
      }

      status = 'loading';
      error = '';
      try {
        monoInfo = await invoke<MonoVolumeInfo>('load_mono_volumes', { paths: monoPaths });
        status = 'loaded';
      } catch (e) {
        error = e instanceof Error ? e.message : String(e);
        status = 'error';
      }
    },

    async runDecomposition() {
      if (status !== 'loaded' && status !== 'complete') {
        error = 'Load mono-energetic volumes first';
        return;
      }

      status = 'running';
      progress = 0;
      error = '';

      const unlisten = await listen<number>('mmd-progress', (event) => {
        progress = Math.round(event.payload * 100);
      });

      try {
        result = await invoke<MmdSummary>('run_mmd', { config });
        status = 'complete';
      } catch (e) {
        error = e instanceof Error ? e.message : String(e);
        status = 'error';
      } finally {
        unlisten();
      }
    },

    clear() {
      status = 'idle';
      monoInfo = null;
      result = null;
      progress = 0;
      error = '';
      monoPaths = {};
    },
  };
}

export const mmdStore = createMmdStore();
