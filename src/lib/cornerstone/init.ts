/**
 * Cornerstone3D initialization.
 *
 * Initializes the core rendering engine and registers interactive tools.
 * Must be called once before any viewport is created.
 */
import { init as csInit, RenderingEngine } from '@cornerstonejs/core';
import {
  init as csToolsInit,
  addTool,
  WindowLevelTool,
  StackScrollTool,
  PanTool,
  ZoomTool,
} from '@cornerstonejs/tools';

const RENDERING_ENGINE_ID = 'pcat-rendering-engine';

let renderingEngine: RenderingEngine | null = null;
let initialized = false;

/**
 * Initialize cornerstone3D core + tools and create the singleton RenderingEngine.
 *
 * Safe to call multiple times — subsequent calls are no-ops.
 */
export async function initCornerstone(): Promise<void> {
  if (initialized) return;

  // Core init (synchronous in v4, returns boolean)
  csInit();

  // Tools init
  csToolsInit();

  // Register tools globally so ToolGroups can reference them by name
  addTool(WindowLevelTool);
  addTool(StackScrollTool);
  addTool(PanTool);
  addTool(ZoomTool);

  // Create the singleton rendering engine
  renderingEngine = new RenderingEngine(RENDERING_ENGINE_ID);

  initialized = true;
}

/**
 * Return the singleton RenderingEngine.
 * Throws if called before `initCornerstone()`.
 */
export function getRenderingEngine(): RenderingEngine {
  if (!renderingEngine) {
    throw new Error(
      'Cornerstone not initialized. Call initCornerstone() first.',
    );
  }
  return renderingEngine;
}

export { RENDERING_ENGINE_ID };
