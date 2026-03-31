<script lang="ts">
  /**
   * Multi-Material Decomposition panel.
   *
   * Allows loading 4 mono-energetic volumes, configuring basis LACs,
   * running decomposition, and viewing summary results.
   */
  import { mmdStore } from '$lib/stores/mmdStore.svelte';
  import { invoke } from '@tauri-apps/api/core';

  const energies = ['70', '100', '140', '150'];

  async function pickFolder(keV: string) {
    const path = await invoke<string | null>('open_dicom_dialog');
    if (path) {
      mmdStore.setMonoPath(keV, path);
    }
  }

  async function loadAll() {
    await mmdStore.loadMonoVolumes();
  }

  async function runMmd() {
    await mmdStore.runDecomposition();
  }

  function formatFraction(v: number): string {
    return (v * 100).toFixed(1) + '%';
  }
</script>

<div class="flex flex-col gap-3 p-4 text-xs text-text-primary">
  <h2 class="text-sm font-semibold">Multi-Material Decomposition</h2>
  <p class="text-[11px] text-text-secondary">
    Decompose mono-energetic CT into water, lipid, and iodine volume fractions.
  </p>

  <!-- Mono-energy volume loaders -->
  <div class="flex flex-col gap-1.5">
    <span class="text-[10px] font-semibold uppercase tracking-wider text-text-secondary/60">
      Mono-Energetic Volumes
    </span>
    {#each energies as keV}
      <div class="flex items-center gap-2">
        <span class="w-12 text-right tabular-nums">{keV} keV</span>
        <button
          class="flex-1 truncate rounded border border-border px-2 py-1 text-left text-[11px] hover:bg-surface-tertiary"
          onclick={() => pickFolder(keV)}
          disabled={mmdStore.status === 'running'}
        >
          {mmdStore.monoPaths[keV]
            ? mmdStore.monoPaths[keV].split('/').slice(-2).join('/')
            : 'Select folder...'}
        </button>
      </div>
    {/each}
  </div>

  <!-- Load button -->
  <button
    class="rounded bg-accent/10 px-3 py-1.5 text-xs font-medium text-accent hover:bg-accent/20 disabled:opacity-40"
    onclick={loadAll}
    disabled={Object.keys(mmdStore.monoPaths).length < 4 || mmdStore.status === 'running' || mmdStore.status === 'loading'}
  >
    {mmdStore.status === 'loading' ? 'Loading...' : 'Load Volumes'}
  </button>

  <!-- Volume info -->
  {#if mmdStore.monoInfo}
    <div class="rounded border border-border/50 bg-surface-tertiary/50 px-3 py-2 text-[11px]">
      <div>Energies: {mmdStore.monoInfo.energies.join(', ')} keV</div>
      <div>Shape: {mmdStore.monoInfo.shape.join(' x ')}</div>
      <div>Spacing: {mmdStore.monoInfo.spacing.map(s => s.toFixed(2)).join(' x ')} mm</div>
    </div>
  {/if}

  <!-- Run decomposition -->
  {#if mmdStore.status === 'loaded' || mmdStore.status === 'complete'}
    <button
      class="rounded bg-accent px-3 py-1.5 text-xs font-medium text-white hover:bg-accent/90 disabled:opacity-40"
      onclick={runMmd}
      disabled={mmdStore.status === 'running'}
    >
      {mmdStore.status === 'running' ? `Decomposing... ${mmdStore.progress}%` : 'Run MMD'}
    </button>
  {/if}

  <!-- Progress bar -->
  {#if mmdStore.status === 'running'}
    <div class="h-1.5 w-full overflow-hidden rounded-full bg-surface-tertiary">
      <div
        class="h-full rounded-full bg-accent transition-all duration-150"
        style="width: {mmdStore.progress}%"
      ></div>
    </div>
  {/if}

  <!-- Results -->
  {#if mmdStore.result}
    <div class="flex flex-col gap-1 rounded border border-success/30 bg-success/5 px-3 py-2 text-[11px]">
      <span class="font-semibold text-success">Decomposition Complete</span>
      <div>Elapsed: {mmdStore.result.elapsed_ms} ms</div>
      <div class="mt-1 grid grid-cols-2 gap-x-4 gap-y-0.5">
        <span class="text-text-secondary">Mean water:</span>
        <span class="tabular-nums">{formatFraction(mmdStore.result.mean_water)}</span>
        <span class="text-text-secondary">Mean lipid:</span>
        <span class="tabular-nums">{formatFraction(mmdStore.result.mean_lipid)}</span>
        <span class="text-text-secondary">Mean iodine:</span>
        <span class="tabular-nums">{formatFraction(mmdStore.result.mean_iodine)}</span>
        <span class="text-text-secondary">Mean residual:</span>
        <span class="tabular-nums">{mmdStore.result.mean_residual.toExponential(2)}</span>
      </div>
    </div>
  {/if}

  <!-- Error -->
  {#if mmdStore.error}
    <div class="rounded border border-error/30 bg-error/5 px-3 py-2 text-[11px] text-error">
      {mmdStore.error}
    </div>
  {/if}
</div>
