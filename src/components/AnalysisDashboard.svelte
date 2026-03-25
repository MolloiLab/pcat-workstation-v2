<script lang="ts">
  /**
   * FAI Analysis Dashboard — tabbed panel for the bottom-right context area.
   *
   * Tabs:
   *   1. Overview — per-vessel FAI summary cards
   *   2. Histograms — Plotly.js HU distribution charts
   *   3. CPR + FAI — placeholder for Phase 4
   */
  import { onMount, tick } from 'svelte';
  import { pipelineStore, type FaiStats } from '$lib/stores/pipelineStore.svelte';
  import { VESSEL_COLORS, type Vessel } from '$lib/stores/seedStore.svelte';

  const TABS = ['Overview', 'Histograms', 'CPR + FAI'] as const;
  type Tab = (typeof TABS)[number];

  let activeTab = $state<Tab>('Overview');
  let chartDiv: HTMLDivElement | undefined = $state();
  let plotlyLoaded = $state(false);
  let Plotly: typeof import('plotly.js-dist-min') | null = $state(null);

  const vesselOrder: Vessel[] = ['LAD', 'LCx', 'RCA'];

  let vesselResults = $derived(
    pipelineStore.results
      ? vesselOrder
          .filter((v) => v in pipelineStore.results!)
          .map((v) => ({ vessel: v, stats: pipelineStore.results![v] }))
      : [],
  );

  // Lazy-load Plotly to avoid blocking initial render
  onMount(async () => {
    const mod = await import('plotly.js-dist-min');
    Plotly = mod.default ?? mod;
    plotlyLoaded = true;
  });

  // Render chart when tab switches to Histograms and we have data + Plotly
  $effect(() => {
    if (activeTab !== 'Histograms' || !plotlyLoaded || !Plotly) return;
    if (!pipelineStore.results || vesselResults.length === 0) return;

    // Wait for DOM to have the chart div
    tick().then(() => {
      if (!chartDiv || !Plotly) return;
      renderHistogram(Plotly, chartDiv, vesselResults);
    });
  });

  function renderHistogram(
    P: typeof import('plotly.js-dist-min'),
    div: HTMLDivElement,
    data: { vessel: Vessel; stats: FaiStats }[],
  ) {
    const traces: Partial<Plotly.Data>[] = data.map(({ vessel, stats }) => ({
      x: stats.histogram_bins,
      y: stats.histogram_counts,
      type: 'bar' as const,
      name: vessel,
      marker: {
        color: VESSEL_COLORS[vessel],
        opacity: 0.7,
      },
    }));

    const shapes: Partial<Plotly.Shape>[] = [
      // FAI window: -190 HU
      {
        type: 'line',
        x0: -190,
        x1: -190,
        y0: 0,
        y1: 1,
        yref: 'paper',
        line: { color: '#98989d', width: 1, dash: 'dash' },
      },
      // FAI window: -30 HU
      {
        type: 'line',
        x0: -30,
        x1: -30,
        y0: 0,
        y1: 1,
        yref: 'paper',
        line: { color: '#98989d', width: 1, dash: 'dash' },
      },
      // Risk threshold: -70.1 HU
      {
        type: 'line',
        x0: -70.1,
        x1: -70.1,
        y0: 0,
        y1: 1,
        yref: 'paper',
        line: { color: '#ff453a', width: 2, dash: 'dot' },
      },
    ];

    const annotations: Partial<Plotly.Annotations>[] = [
      {
        x: -190,
        y: 1,
        yref: 'paper',
        text: '-190',
        showarrow: false,
        font: { color: '#98989d', size: 9 },
        yanchor: 'bottom',
      },
      {
        x: -30,
        y: 1,
        yref: 'paper',
        text: '-30',
        showarrow: false,
        font: { color: '#98989d', size: 9 },
        yanchor: 'bottom',
      },
      {
        x: -70.1,
        y: 1,
        yref: 'paper',
        text: '-70.1 (risk)',
        showarrow: false,
        font: { color: '#ff453a', size: 9 },
        yanchor: 'bottom',
      },
    ];

    const layout: Partial<Plotly.Layout> = {
      paper_bgcolor: '#1c1c1e',
      plot_bgcolor: '#2c2c2e',
      font: { color: '#e5e5e7', size: 10 },
      margin: { l: 45, r: 15, t: 30, b: 40 },
      title: {
        text: 'HU Distribution (FAI Window)',
        font: { size: 12, color: '#e5e5e7' },
      },
      xaxis: {
        title: { text: 'HU', font: { size: 10 } },
        gridcolor: '#38383a',
        zerolinecolor: '#38383a',
      },
      yaxis: {
        title: { text: 'Count', font: { size: 10 } },
        gridcolor: '#38383a',
        zerolinecolor: '#38383a',
      },
      barmode: 'overlay',
      shapes: shapes as Plotly.Layout['shapes'],
      annotations: annotations as Plotly.Layout['annotations'],
      legend: {
        font: { color: '#e5e5e7', size: 10 },
        bgcolor: 'rgba(0,0,0,0)',
      },
      autosize: true,
    };

    const config: Partial<Plotly.Config> = {
      responsive: true,
      displayModeBar: false,
    };

    // Use any to avoid strict type issues with Plotly typings
    (P as any).newPlot(div, traces, layout, config);
  }
</script>

<div class="flex h-full w-full flex-col bg-surface-secondary">
  <!-- Tab bar -->
  <div class="flex shrink-0 border-b border-border">
    {#each TABS as tab}
      <button
        class="px-3 py-2 text-[11px] font-medium transition-colors"
        class:text-accent={activeTab === tab}
        class:border-b-2={activeTab === tab}
        class:border-accent={activeTab === tab}
        class:text-text-secondary={activeTab !== tab}
        class:hover:text-text-primary={activeTab !== tab}
        onclick={() => (activeTab = tab)}
      >
        {tab}
      </button>
    {/each}
  </div>

  <!-- Tab content -->
  <div class="min-h-0 flex-1 overflow-y-auto">
    {#if activeTab === 'Overview'}
      <!-- Overview: per-vessel FAI summary cards -->
      <div class="flex flex-col gap-2.5 p-3">
        {#if vesselResults.length === 0}
          <div class="flex flex-1 items-center justify-center py-8">
            <p class="text-xs text-text-secondary">No results available</p>
          </div>
        {:else}
          {#each vesselResults as { vessel, stats }}
            {@const isHigh = stats.fai_risk === 'HIGH'}
            <div
              class="rounded-lg border border-border bg-surface p-3"
            >
              <!-- Vessel header with risk badge -->
              <div class="mb-2 flex items-center justify-between">
                <div class="flex items-center gap-2">
                  <span
                    class="h-2.5 w-2.5 rounded-full"
                    style="background-color: {VESSEL_COLORS[vessel]}"
                  ></span>
                  <span class="text-xs font-semibold text-text-primary">
                    {vessel}
                  </span>
                </div>
                <span
                  class="rounded px-1.5 py-0.5 text-[10px] font-bold"
                  style="background-color: {isHigh ? 'rgba(255,69,58,0.2)' : 'rgba(48,209,88,0.2)'}; color: {isHigh ? 'var(--color-error)' : 'var(--color-success)'}"
                >
                  {stats.fai_risk}
                </span>
              </div>

              <!-- FAI mean HU (hero number) -->
              <div class="mb-2">
                <span class="text-[10px] text-text-secondary">Mean HU</span>
                <p
                  class="text-xl font-bold tabular-nums"
                  style="color: {isHigh ? 'var(--color-error)' : 'var(--color-success)'}"
                >
                  {stats.hu_mean.toFixed(1)}
                </p>
              </div>

              <!-- Stats grid -->
              <div class="grid grid-cols-3 gap-2">
                <div>
                  <span class="text-[9px] text-text-secondary">Median HU</span>
                  <p class="text-[11px] tabular-nums text-text-primary">
                    {stats.hu_median.toFixed(1)}
                  </p>
                </div>
                <div>
                  <span class="text-[9px] text-text-secondary">Std HU</span>
                  <p class="text-[11px] tabular-nums text-text-primary">
                    {stats.hu_std.toFixed(1)}
                  </p>
                </div>
                <div>
                  <span class="text-[9px] text-text-secondary">Fat %</span>
                  <p class="text-[11px] tabular-nums text-text-primary">
                    {(stats.fat_fraction * 100).toFixed(1)}%
                  </p>
                </div>
                <div>
                  <span class="text-[9px] text-text-secondary">VOI Voxels</span>
                  <p class="text-[11px] tabular-nums text-text-primary">
                    {stats.n_voi_voxels.toLocaleString()}
                  </p>
                </div>
                <div>
                  <span class="text-[9px] text-text-secondary">Fat Voxels</span>
                  <p class="text-[11px] tabular-nums text-text-primary">
                    {stats.n_fat_voxels.toLocaleString()}
                  </p>
                </div>
              </div>
            </div>
          {/each}
        {/if}
      </div>
    {:else if activeTab === 'Histograms'}
      <!-- Histograms: Plotly.js chart -->
      <div class="flex h-full w-full items-center justify-center p-2">
        {#if !plotlyLoaded}
          <p class="text-xs text-text-secondary">Loading chart library...</p>
        {:else if vesselResults.length === 0}
          <p class="text-xs text-text-secondary">No histogram data available</p>
        {:else}
          <div
            bind:this={chartDiv}
            class="h-full w-full"
            style="min-height: 200px;"
          ></div>
        {/if}
      </div>
    {:else if activeTab === 'CPR + FAI'}
      <!-- CPR + FAI: Placeholder -->
      <div class="flex flex-1 items-center justify-center py-12">
        <div class="flex flex-col items-center gap-2">
          <svg
            class="h-8 w-8 text-text-secondary/30"
            fill="none"
            stroke="currentColor"
            stroke-width="1.5"
            viewBox="0 0 24 24"
          >
            <path
              stroke-linecap="round"
              stroke-linejoin="round"
              d="M9.53 16.122a3 3 0 0 0-5.78 1.128 2.25 2.25 0 0 1-2.4 2.245 4.5 4.5 0 0 0 8.4-2.245c0-.399-.078-.78-.22-1.128Zm0 0a15.998 15.998 0 0 0 3.388-1.62m-5.043-.025a15.994 15.994 0 0 1 1.622-3.395m3.42 3.42a15.995 15.995 0 0 0 4.764-4.648l3.876-5.814a1.151 1.151 0 0 0-1.597-1.597L14.146 6.32a15.996 15.996 0 0 0-4.649 4.763m3.42 3.42a6.776 6.776 0 0 0-3.42-3.42"
            />
          </svg>
          <p class="text-xs text-text-secondary">
            CPR with FAI overlay
          </p>
          <p class="text-[10px] text-text-secondary/50">
            Coming in Phase 4
          </p>
        </div>
      </div>
    {/if}
  </div>
</div>
