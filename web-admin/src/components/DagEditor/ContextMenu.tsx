import { useCallback, useEffect, useRef } from 'react';

interface ContextMenuProps {
  x: number;
  y: number;
  nodeId: string;
  functionId?: number | null;
  isStartNode?: boolean;
  isEndNode?: boolean;
  isAnswerNode?: boolean;
  onDelete: (nodeId: string) => void;
  onViewFunction?: (functionId: number) => void;
  onClose: () => void;
}

export function ContextMenu({ x, y, nodeId, functionId, isStartNode, isEndNode, isAnswerNode, onDelete, onViewFunction, onClose }: ContextMenuProps) {
  const menuRef = useRef<HTMLDivElement>(null);

  const handleClickOutside = useCallback((event: MouseEvent) => {
    if (menuRef.current && !menuRef.current.contains(event.target as Node)) {
      onClose();
    }
  }, [onClose]);

  useEffect(() => {
    document.addEventListener('mousedown', handleClickOutside);
    return () => {
      document.removeEventListener('mousedown', handleClickOutside);
    };
  }, [handleClickOutside]);

  const isVirtualNode = isStartNode || isEndNode;
  const isAnswer = isAnswerNode;

  return (
    <div
      ref={menuRef}
      style={{
        position: 'fixed',
        top: y,
        left: x,
        background: '#fff',
        border: '1px solid #e8e8e8',
        borderRadius: 4,
        boxShadow: '0 4px 16px rgba(0, 0, 0, 0.15)',
        padding: 4,
        zIndex: 9999,
        minWidth: 120,
      }}
    >
      {onViewFunction && functionId && !isVirtualNode && !isAnswer && (
        <button
          onClick={() => {
            onViewFunction(functionId);
            onClose();
          }}
          style={{
            width: '100%',
            padding: '8px 12px',
            textAlign: 'left',
            border: 'none',
            background: 'none',
            cursor: 'pointer',
            fontSize: 14,
            color: '#333',
            borderRadius: 4,
          }}
          onMouseEnter={(e) => {
            (e.target as HTMLElement).style.background = '#f5f5f5';
          }}
          onMouseLeave={(e) => {
            (e.target as HTMLElement).style.background = 'none';
          }}
        >
          查看函数
        </button>
      )}
      {!isVirtualNode && (
        <button
          onClick={() => {
            onDelete(nodeId);
            onClose();
          }}
          style={{
            width: '100%',
            padding: '8px 12px',
            textAlign: 'left',
            border: 'none',
            background: 'none',
            cursor: 'pointer',
            fontSize: 14,
            color: '#ff4d4f',
            borderRadius: 4,
          }}
          onMouseEnter={(e) => {
            (e.target as HTMLElement).style.background = '#fff2f0';
          }}
          onMouseLeave={(e) => {
            (e.target as HTMLElement).style.background = 'none';
          }}
        >
          删除节点
        </button>
      )}
      {isStartNode && (
        <div
          style={{
            padding: '8px 12px',
            fontSize: 12,
            color: '#999',
          }}
        >
          起始节点
        </div>
      )}
      {isEndNode && (
        <div
          style={{
            padding: '8px 12px',
            fontSize: 12,
            color: '#999',
          }}
        >
          结束节点
        </div>
      )}
    </div>
  );
}