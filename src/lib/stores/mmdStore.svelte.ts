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
  basis_lacs: [[number, number, number], [number, number, number], [number, number, number], [number, number, number]];
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

  /** Default config — user can override via UI. */
  let config: MmdConfig = $state({
    basis_lacs: [
      [0.0193, 0.0171, 0.0800],  // 70 keV
      [0.0171, 0.0159, 0.0250],  // 100 keV
      [0.0155, 0.0148, 0.0130],  // 140 keV
      [0.0152, 0.0146, 0.0120],  // 150 keV
    ],
    noise_variances: [1e-8, 1e-8, 1e-8, 1e-8],
    hu_upper: 150.0,
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
