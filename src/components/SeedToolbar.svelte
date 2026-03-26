<script lang="ts">
  /**
   * Vessel selector toolbar for seed placement.
   *
   * Shows a colored button per vessel with seed count, edit buttons for
   * navigating/deleting seeds, plus a "Clear All" action.
   * Active vessel has a filled background; inactive vessels show an outline.
   */
  import { seedStore, VESSEL_COLORS, type Vessel } from '$lib/stores/seedStore.svelte';
  import { navigateToWorldPos } from '$lib/navigation';

  const vesselNames: Vessel[] = ['RCA', 'LAD', 'LCx'];

  function prevSeed() {
    const data = seedStore.activeVesselData;
    if (data.seeds.length === 0) return;
    const current = seedStore.selectedSeedIndex;
    let next: number;
    if (current === null) {
      next = data.seeds.length - 1;
    } else {
      next = (current - 1 + data.seeds.length) % data.seeds.length;
    }
    seedStore.selectSeed(next);
    navigateToWorldPos(data.seeds[next].position);
  }

  function nextSeed() {
    const data = seedStore.activeVesselData;
    if (data.seeds.length === 0) return;
    const current = seedStore.selectedSeedIndex;
    let next: number;
    if (current === null) {
      next = 0;
    } else {
      next = (current + 1) % data.seeds.length;
    }
    seedStore.selectSeed(next);
    navigateToWorldPos(data.seeds[next].position);
  }

  function deleteSeed() {
    const selected = seedStore.selectedSeedIndex;
    if (selected !== null) {
      seedStore.removeSeed(selected);
    } else {
      const data = seedStore.activeVesselData;
      if (data.seeds.length > 0) {
        seedStore.removeSeed(data.seeds.length - 1);
      }
    }
  }

  function clearVessel() {
    seedStore.clearVessel(seedStore.activeVessel);
  }
</script>

<div class="flex items-center gap-1.5">
  <span class="mr-1 text-[11px] text-text-secondary">Seeds:</span>

  {#each vesselNames as vessel}
    {@const data = seedStore.vessels[vessel]}
    {@const color = VESSEL_COLORS[vessel]}
    {@const isActive = seedStore.activeVessel === vessel}
    <button
      class="rounded px-2.5 py-1 text-xs font-medium transition-colors"
      style={isActive
        ? `background-color: ${color}; color: #000;`
        : `background-color: transparent; color: ${color}; box-shadow: inset 0 0 0 1px ${color};`}
      onclick={() => seedStore.setActiveVessel(vessel)}
    >
      {vessel}
      {#if data.seeds.length > 0}
        <span class="ml-0.5 opacity-70">({data.seeds.length})</span>
      {/if}
    </button>
  {/each}

  {#if seedStore.activeVesselData.seeds.length > 0}
    <div class="flex items-center gap-0.5 ml-2 border-l border-border pl-2">
      <button
        class="rounded px-1.5 py-0.5 text-[11px] text-text-secondary hover:bg-surface-tertiary hover:text-text-primary"
        onclick={prevSeed}
        title="Previous seed (Left Arrow)"
      >
        &#9664;
      </button>
      <button
        class="rounded px-1.5 py-0.5 text-[11px] text-text-secondary hover:bg-surface-tertiary hover:text-text-primary"
        onclick={nextSeed}
        title="Next seed (Right Arrow)"
      >
        &#9654;
      </button>
      <button
        class="rounded px-1.5 py-0.5 text-[11px] text-text-secondary hover:bg-surface-tertiary hover:text-error"
        onclick={deleteSeed}
        title="Delete selected (Del)"
      >
        &#10005;
      </button>
      <button
        class="rounded px-1.5 py-0.5 text-[11px] text-text-secondary hover:bg-surface-tertiary hover:text-error"
        onclick={clearVessel}
        title="Clear vessel (Esc)"
      >
        Clear
      </button>
    </div>
  {/if}

  {#if seedStore.vessels.LAD.seeds.length > 0 || seedStore.vessels.LCx.seeds.length > 0 || seedStore.vessels.RCA.seeds.length > 0}
    <button
      class="ml-1 rounded px-2 py-1 text-[11px] text-text-secondary hover:bg-surface-tertiary hover:text-error"
      onclick={() => seedStore.clearAll()}
    >
      Clear All
    </button>
  {/if}
</div>
