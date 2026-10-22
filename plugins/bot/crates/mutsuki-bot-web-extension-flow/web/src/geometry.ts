import type { TypeRef } from "./contract";

export const NODE_WIDTH = 220;
export const NODE_HEADER = 71;
export const PORT_ROW = 28;
export const ERROR_TYPE: TypeRef = { type_id: "mutsuki.bot.flow.error", version: 1 };

export interface Point {
  x: number;
  y: number;
}

export function edgePath(from: Point, to: Point): string {
  const curve = Math.max(48, Math.abs(to.x - from.x) / 2);
  return `M ${from.x} ${from.y} C ${from.x + curve} ${from.y}, ${to.x - curve} ${to.y}, ${to.x} ${to.y}`;
}

/** Same type and version, or a concrete event subtype into the generic `mutsuki.bot.event` input. */
export function samePortType(left: TypeRef | null | undefined, right: TypeRef | null | undefined): boolean {
  if (!left || !right || Number(left.version) !== Number(right.version)) return false;
  if (left.type_id === right.type_id) return true;
  return right.type_id === "mutsuki.bot.event" && left.type_id.startsWith("mutsuki.bot.event.");
}

/** Port position from the fixed card layout; the canvas prefers measured positions. */
export function portAnchor(node: Point, direction: "input" | "output", index: number): Point {
  return { x: direction === "output" ? node.x + NODE_WIDTH : node.x, y: node.y + NODE_HEADER + index * PORT_ROW };
}

export interface View {
  x: number;
  y: number;
  scale: number;
}

export function clientToWorld(worldRect: { left: number; top: number }, view: View, clientX: number, clientY: number): Point {
  return { x: (clientX - worldRect.left) / view.scale, y: (clientY - worldRect.top) / view.scale };
}

/** Zoom around a client point, clamped to the editor's range. */
export function zoomAt(view: View, worldRect: { left: number; top: number }, clientX: number, clientY: number, zoomIn: boolean): View {
  const point = clientToWorld(worldRect, view, clientX, clientY);
  const scale = Math.min(1.8, Math.max(0.55, view.scale * (zoomIn ? 1.08 : 0.92)));
  return { x: view.x + (clientX - worldRect.left) - point.x * scale, y: view.y + (clientY - worldRect.top) - point.y * scale, scale };
}
