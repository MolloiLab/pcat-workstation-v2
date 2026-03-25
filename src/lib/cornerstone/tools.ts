/**
 * Tool-group configuration for the MPR viewports.
 *
 * Bindings:
 *   Right-drag   → Window/Level
 *   Middle-drag   → Pan
 *   Scroll        → Stack Scroll (slice navigation)
 *   Ctrl + Left   → Zoom
 */
import {
  ToolGroupManager,
  WindowLevelTool,
  StackScrollTool,
  PanTool,
  ZoomTool,
  Enums as csToolsEnums,
} from '@cornerstonejs/tools';

import { RENDERING_ENGINE_ID } from './init';

const { MouseBindings, KeyboardBindings } = csToolsEnums;

const TOOL_GROUP_ID = 'pcat-mpr-tool-group';

/**
 * Create (or retrieve) the MPR tool group and bind it to the given viewports.
 *
 * @param viewportIds - IDs of viewports that should share this tool group.
 * @returns The configured tool group.
 */
export function setupToolGroup(viewportIds: string[]) {
  // Reuse existing group if already created
  let toolGroup = ToolGroupManager.getToolGroup(TOOL_GROUP_ID);

  if (!toolGroup) {
    toolGroup = ToolGroupManager.createToolGroup(TOOL_GROUP_ID);
    if (!toolGroup) {
      throw new Error('Failed to create tool group');
    }

    // Add tools to the group
    toolGroup.addTool(WindowLevelTool.toolName);
    toolGroup.addTool(StackScrollTool.toolName);
    toolGroup.addTool(PanTool.toolName);
    toolGroup.addTool(ZoomTool.toolName);

    // --- Activate with mouse bindings ---

    // Right-drag → Window/Level
    toolGroup.setToolActive(WindowLevelTool.toolName, {
      bindings: [{ mouseButton: MouseBindings.Secondary }],
    });

    // Middle-drag → Pan
    toolGroup.setToolActive(PanTool.toolName, {
      bindings: [{ mouseButton: MouseBindings.Auxiliary }],
    });

    // Scroll → Stack Scroll (slice navigation)
    toolGroup.setToolActive(StackScrollTool.toolName, {
      bindings: [{ mouseButton: MouseBindings.Wheel }],
    });

    // Ctrl + Left-click → Zoom
    toolGroup.setToolActive(ZoomTool.toolName, {
      bindings: [
        {
          mouseButton: MouseBindings.Primary,
          modifierKey: KeyboardBindings.Ctrl,
        },
      ],
    });
  }

  // Attach viewports to the tool group
  for (const viewportId of viewportIds) {
    toolGroup.addViewport(viewportId, RENDERING_ENGINE_ID);
  }

  return toolGroup;
}

export { TOOL_GROUP_ID };
