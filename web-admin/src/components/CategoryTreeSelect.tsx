import { useEffect, useState } from 'react';
import { TreeSelect, Spin } from 'antd';
import type { DefaultOptionType } from 'antd/es/select';
import { listCategoriesTree, listCategoriesFlat, type CategoryNode, type CategoryItem } from '../services/category';

function toTreeSelectOptions(nodes: CategoryNode[]): DefaultOptionType[] {
  return nodes.map((n) => ({
    value: n.id,
    label: n.name,
    children: n.children?.length ? toTreeSelectOptions(n.children) : undefined,
  }));
}

export interface CategoryTreeSelectProps {
  value?: number | null;
  onChange?: (value: number | null | undefined) => void;
  placeholder?: string;
  allowClear?: boolean;
  showSearch?: boolean;
}

export function CategoryTreeSelect({
  value,
  onChange,
  placeholder = '选择分类',
  allowClear = true,
  showSearch = true,
}: CategoryTreeSelectProps) {
  const [treeData, setTreeData] = useState<DefaultOptionType[]>([]);
  const [flatData, setFlatData] = useState<CategoryItem[]>([]);
  const [loading, setLoading] = useState(false);

  const handleChange = (val: any) => {
    if (val === undefined || val === null) {
      onChange?.(null);
    } else if (typeof val === 'string') {
      onChange?.(Number(val));
    } else {
      onChange?.(val);
    }
  };

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    listCategoriesTree()
      .then((tree) => {
        if (!cancelled) {
          setTreeData(toTreeSelectOptions(tree));
        }
      })
      .catch(() => {
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    listCategoriesFlat()
      .then((flat) => {
        if (!cancelled) setFlatData(flat);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, []);

  const buildSearchOptions = (searchValue: string): DefaultOptionType[] => {
    if (!searchValue) return treeData;
    const lower = searchValue.toLowerCase();
    const matched = flatData.filter(
      (item) => item.name.toLowerCase().includes(lower) || item.slug.toLowerCase().includes(lower),
    );
    const matchedIds = new Set(matched.map((m) => m.id));
    const filterNodes = (nodes: DefaultOptionType[]): DefaultOptionType[] =>
      nodes
        .map((node) => {
          if (matchedIds.has(node.value as number)) return node;
          if (node.children) {
            const filtered = filterNodes(node.children);
            if (filtered.length > 0) return { ...node, children: filtered };
          }
          return null;
        })
        .filter((n): n is DefaultOptionType => n !== null);
    return filterNodes(treeData);
  };

  if (loading) {
    return <Spin size="small" />;
  }

  return (
    <TreeSelect
      value={value ?? undefined}
      onChange={handleChange}
      treeData={treeData as any}
      placeholder={placeholder}
      allowClear={allowClear}
      showSearch={showSearch}
      treeNodeFilterProp="label"
      filterTreeNode={(inputValue, treeNode) => {
        const label = String(treeNode.label ?? '').toLowerCase();
        return label.includes(inputValue.toLowerCase());
      }}
      treeDefaultExpandAll={false}
      style={{ width: '100%' }}
    />
  );
}
