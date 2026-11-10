export type Box = { x: number; y: number; w: number; h: number };
export function boxesIntersect(a: Box, b: Box) {
  return (
    a.x < b.x + b.w && a.x + a.w > b.x && a.y < b.y + b.h && a.y + a.h > b.y
  );
}
