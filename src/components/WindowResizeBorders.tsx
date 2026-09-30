import React from "react";
import { isTauriRuntime } from "../lib/platform";
import { getTauriWindow } from "../lib/tauriWindow";

type ResizeDirection =
  | "East"
  | "North"
  | "NorthEast"
  | "NorthWest"
  | "South"
  | "SouthEast"
  | "SouthWest"
  | "West";

interface WindowResizeBordersProps {
  isMaximized?: boolean;
}

const isMacOs =
  typeof navigator !== "undefined" &&
  /(Mac|iPhone|iPod|iPad)/i.test(navigator.userAgent);

export const WindowResizeBorders: React.FC<WindowResizeBordersProps> = ({
  isMaximized = false,
}) => {
  if (!isTauriRuntime() || isMacOs || isMaximized) {
    return null;
  }

  const handleResizeStart =
    (direction: ResizeDirection) => (event: React.MouseEvent) => {
      if (event.button !== 0) return;
      event.preventDefault();
      event.stopPropagation();
      void getTauriWindow()?.startResizeDragging(direction);
    };

  return (
    <div
      className="pointer-events-none fixed inset-0 z-50 overflow-hidden"
      aria-hidden="true"
    >
      {/* Edges */}
      <div
        onMouseDown={handleResizeStart("North")}
        className="pointer-events-auto absolute top-0 left-3 right-3 h-1.5 cursor-n-resize select-none"
      />
      <div
        onMouseDown={handleResizeStart("South")}
        className="pointer-events-auto absolute bottom-0 left-3 right-3 h-1.5 cursor-s-resize select-none"
      />
      <div
        onMouseDown={handleResizeStart("West")}
        className="pointer-events-auto absolute top-3 bottom-3 left-0 w-1.5 cursor-w-resize select-none"
      />
      <div
        onMouseDown={handleResizeStart("East")}
        className="pointer-events-auto absolute top-3 bottom-3 right-0 w-1.5 cursor-e-resize select-none"
      />

      {/* Corners */}
      <div
        onMouseDown={handleResizeStart("NorthWest")}
        className="pointer-events-auto absolute top-0 left-0 h-3 w-3 cursor-nw-resize select-none"
      />
      <div
        onMouseDown={handleResizeStart("NorthEast")}
        className="pointer-events-auto absolute top-0 right-0 h-3 w-3 cursor-ne-resize select-none"
      />
      <div
        onMouseDown={handleResizeStart("SouthWest")}
        className="pointer-events-auto absolute bottom-0 left-0 h-3 w-3 cursor-sw-resize select-none"
      />
      <div
        onMouseDown={handleResizeStart("SouthEast")}
        className="pointer-events-auto absolute bottom-0 right-0 h-3 w-3 cursor-se-resize select-none"
      />
    </div>
  );
};
