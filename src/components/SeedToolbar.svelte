<script lang="ts">
  /**
   * Vessel selector toolbar for seed placement.
   *
   * Shows a colored button per vessel with seed count, plus a "Clear All" action.
   * Active vessel has a filled background; inactive vessels show an outline.
   */
  import { seedStore, VESSEL_COLORS, type Vessel } from '$lib/stores/seedStore.svelte';

  const vesselNames: Vessel[] = ['RCA', 'LAD', 'LCx'];
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

  {#if seedStore.vessels.LAD.seeds.length > 0 || seedStore.vessels.LCx.seeds.length > 0 || seedStore.vessels.RCA.seeds.length > 0}
    <button
      class="ml-1 rounded px-2 py-1 text-[11px] text-text-secondary hover:bg-surface-tertiary hover:text-error"
      onclick={() => seedStore.clearAll()}
    >
      Clear All
    </button>
  {/if}
</div>
