<script lang="ts">
  /**
   * Material map viewer: renders water, lipid, and iodine fraction maps
   * as heatmaps using a jet colormap (blue=0%, red=100%).
   *
   * Shows 3 panels (one per material) with a shared axial slice slider.
   */
  import { mmdStore } from '$lib/stores/mmdStore.svelte';
  import { invoke } from '@tauri-apps/api/core';

  const materials = [
    { key: 'water', label: 'Water', color: '#60a5fa' },
    { key: 'lipid', label: 'Lipid', color: '#fbbf24' },
    { key: 'iodine', label: 'Iodine', color: '#f87171' },
  ] as const;

  let sliceIdx = $state(0);
  let maxSlice = $derived(mmdStore.result ? mmdStore.result.shape[0] - 1 : 0);
  let canvasRefs: Record<string, HTMLCanvasElement | null> = $state({ water: null, lipid: null, iodine: null });
  let loading = $state(false);

  // Jet colormap: 0.0 → blue, 0.25 → cyan, 0.5 → green/yellow, 0.75 → orange, 1.0 → red
  function jet(t: number): [number, number, number] {
    const v = Math.max(0, Math.min(1, t));
    let r: number, g: number, b: number;

    if (v < 0.125) {
      r = 0; g = 0; b = 0.5 + v * 4;
    } else if (v < 0.375) {
      r = 0; g = (v - 0.125) * 4; b = 1;
    } else if (v < 0.625) {
      r = (v - 0.375) * 4; g = 1; b = 1 - (v - 0.375) * 4;
    } else if (v < 0.875) {
      r = 1; g = 1 - (v - 0.625) * 4; b = 0;
    } else {
      r = 1 - (v - 0.875) * 4; g = 0; b = 0;
    }

    return [Math.round(r * 255), Math.round(g * 255), Math.round(b * 255)];
  }

  async function renderSlice(material: string, canvas: HTMLCanvasElement) {
    if (!mmdStore.result) return;

    const shape = mmdStore.result.shape;
    const ny = shape[1];
    const nx = shape[2];

    // Fetch raw f32 slice data from Rust.
    const bytes = await invoke<number[]>('get_mmd_slice', {
      material,
      axis: 'axial',
      idx: sliceIdx,
    });

    // Convert number[] → Float32Array.
    const u8 = new Uint8Array(bytes);
    const floats = new Float32Array(u8.buffer);

    // Set canvas size.
    canvas.width = nx;
    canvas.height = ny;

    const ctx = canvas.getContext('2d')!;
    const imgData = ctx.createImageData(nx, ny);

    for (let i = 0; i < floats.length; i++) {
      const [r, g, b] = jet(floats[i]);
      imgData.data[i * 4] = r;
      imgData.data[i * 4 + 1] = g;
      imgData.data[i * 4 + 2] = b;
      imgData.data[i * 4 + 3] = 255;
    }

    ctx.putImageData(imgData, 0, 0);
  }

  async function renderAll() {
    if (!mmdStore.result) return;
    loading = true;
    try {
      await Promise.all(
        materials.map((m) => {
          const canvas = canvasRefs[m.key];
          if (canvas) return renderSlice(m.key, canvas);
        })
      );
    } finally {
      loading = false;
    }
  }

  // Re-render when slice index changes or result becomes available.
  $effect(() => {
    if (mmdStore.result && sliceIdx >= 0) {
      renderAll();
    }
  });

  // Initialize slice to middle when result arrives.
  $effect(() => {
    if (mmdStore.result) {
      sliceIdx = Math.floor(mmdStore.result.shape[0] / 2);
    }
  });

  function handleWheel(e: WheelEvent) {
    e.preventDefault();
    const delta = e.deltaY > 0 ? 1 : -1;
    sliceIdx = Math.max(0, Math.min(maxSlice, sliceIdx + delta));
  }
</script>

{#if mmdStore.result}
  <div class="flex h-full flex-col bg-black">
    <!-- Material map canvases -->
    <div class="flex min-h-0 flex-1">
      {#each materials as mat}
        <div class="relative flex min-w-0 flex-1 flex-col border-r border-border/30 last:border-r-0">
          <!-- Label -->
          <div class="absolute left-2 top-2 z-10 rounded bg-black/60 px-2 py-0.5 text-[11px] font-medium" style="color: {mat.color}">
            {mat.label}
          </div>
          <!-- Canvas -->
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <div class="flex min-h-0 flex-1 items-center justify-center" onwheel={handleWheel}>
            <canvas
              bind:this={canvasRefs[mat.key]}
              class="max-h-full max-w-full object-contain"
              style="image-rendering: pixelated;"
            ></canvas>
          </div>
        </div>
      {/each}
    </div>

    <!-- Colorbar -->
    <div class="flex items-center gap-2 px-4 py-1">
      <span class="text-[10px] text-text-secondary">0%</span>
      <div class="h-2.5 flex-1 rounded" style="background: linear-gradient(to right, #0000cc, #00ccff, #00ff00, #ffff00, #ff6600, #cc0000);"></div>
      <span class="text-[10px] text-text-secondary">100%</span>
    </div>

    <!-- Slice slider -->
    <div class="flex items-center gap-3 border-t border-border/30 px-4 py-2">
      <span class="text-[11px] text-text-secondary">Slice</span>
      <input
        type="range"
        min="0"
        max={maxSlice}
        bind:value={sliceIdx}
        class="flex-1"
      />
      <span class="w-12 text-right text-[11px] tabular-nums text-text-secondary">
        {sliceIdx} / {maxSlice}
      </span>
    </div>
  </div>
{:else}
  <!-- Empty state -->
  <div class="flex h-full items-center justify-center text-text-secondary/40">
    <div class="text-center">
      <div class="text-4xl">&#x1f9ea;</div>
      <p class="mt-2 text-sm">Run MMD to view material maps</p>
    </div>
  </div>
{/if}
