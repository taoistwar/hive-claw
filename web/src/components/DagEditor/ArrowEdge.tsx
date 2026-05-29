import { BaseEdge, getBezierPath, type EdgeProps } from 'reactflow';

export function ArrowEdge({
  id,
  sourceX,
  sourceY,
  targetX,
  targetY,
  sourcePosition,
  targetPosition,
  markerStart,
  markerEnd,
  style,
  selected,
}: EdgeProps) {
  const [edgePath] = getBezierPath({
    sourceX,
    sourceY,
    sourcePosition,
    targetX,
    targetY,
    targetPosition,
  });

  const currentColor = selected ? '#1890ff' : (style?.stroke as string) || '#555';

  return (
    <BaseEdge
      id={id}
      path={edgePath}
      markerStart={markerStart}
      markerEnd={markerEnd}
      style={{
        ...style,
        stroke: currentColor,
        strokeWidth: selected ? 2.5 : (style?.strokeWidth as number) || 2,
      }}
    />
  );
}
