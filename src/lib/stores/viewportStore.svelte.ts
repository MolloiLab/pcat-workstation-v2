/**
 * Reactive store for shared viewport state (crosshair position, window/level).
 *
 * Uses Svelte 5 runes (`$state`) for fine-grained reactivity.
 */

export type CrosshairPosition = {
  /** World coordinate along the X (left-right) axis. */
  worldX: number;
  /** World coordinate along the Y (anterior-posterior) axis. */
  worldY: number;
  /** World coordinate along the Z (superior-inferior) axis. */
  worldZ: number;
};

let crosshair = $state<CrosshairPosition>({ worldX: 0, worldY: 0, worldZ: 0 });
let windowLevel = $state({ center: 40, width: 400 });

export const viewportStore = {
  get crosshair() {
    return crosshair;
  },
  get windowLevel() {
    return windowLevel;
  },

  setCrosshair(pos: CrosshairPosition) {
    crosshair = pos;
  },
  setWindowLevel(center: number, width: number) {
    windowLevel = { center, width };
  },
};
