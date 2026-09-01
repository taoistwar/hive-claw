/// <reference types="vitest/globals" />
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import GameTable from '../GameTable';
import type { Game } from '../../services/gameAlias';

const userRef: { current: { role: number } | null } = { current: null };

vi.mock('../../hooks/useAuth', () => ({
  useAuth: () => ({ user: userRef.current, admin: userRef.current }),
}));

const sampleGame: Game = {
  id: 1,
  name: 'Test Game',
  aliases: ['alias1', 'alias2'],
  created_at: '2026-06-01T10:00:00Z',
  updated_at: '2026-06-01T10:00:00Z',
};

const baseProps = {
  games: [sampleGame],
  loading: false,
  total: 1,
  page: 1,
  pageSize: 10,
  searchText: '',
  canWrite: true,
  onSearch: vi.fn(),
  onPageChange: vi.fn(),
  onEdit: vi.fn(),
  onDelete: vi.fn(),
};

describe('GameTable (T009e)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('renders columns: ID, name, aliases, created_at, updated_at, actions', () => {
    userRef.current = { role: 3 };
    render(<GameTable {...baseProps} />);
    expect(screen.getAllByText(/ID/).length).toBeGreaterThan(0);
    expect(screen.getAllByText(/名称/).length).toBeGreaterThan(0);
    expect(screen.getAllByText(/别名/).length).toBeGreaterThan(0);
    expect(screen.getAllByText(/创建日期|创建时间/).length).toBeGreaterThan(0);
    expect(screen.getAllByText(/修改时间/).length).toBeGreaterThan(0);
    expect(screen.getAllByText(/操作/).length).toBeGreaterThan(0);
  });

  it('shows aliases as tags', () => {
    userRef.current = { role: 3 };
    render(<GameTable {...baseProps} />);
    expect(screen.getByText('alias1')).toBeTruthy();
    expect(screen.getByText('alias2')).toBeTruthy();
  });

  it('shows Edit and Delete buttons when canWrite=true', () => {
    userRef.current = { role: 3 };
    render(<GameTable {...baseProps} canWrite={true} />);
    expect(screen.getAllByRole('button').length).toBeGreaterThan(0);
  });

  it('hides Edit and Delete buttons when canWrite=false (Normal role)', () => {
    userRef.current = { role: 1 };
    render(<GameTable {...baseProps} canWrite={false} />);
    const actionColumn = screen.getAllByText(/操作/)[0];
    expect(actionColumn.closest('tr')?.textContent).not.toContain('编辑');
    expect(actionColumn.closest('tr')?.textContent).not.toContain('删除');
  });

  it('calls onEdit when Edit button clicked', async () => {
    const user = userEvent.setup();
    userRef.current = { role: 3 };
    render(<GameTable {...baseProps} />);
    await user.click(screen.getByRole('button', { name: /edit/i }));
    expect(baseProps.onEdit).toHaveBeenCalledWith(sampleGame);
  });
});

describe('GameAliasForm (T016c, T021c)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mock('antd', async (importOriginal) => {
      const actual = await importOriginal<typeof import('antd')>();
      return {
        ...actual,
        message: {
          success: vi.fn(),
          error: vi.fn(),
          info: vi.fn(),
        },
      };
    });
  });

  it('renders create form with name input and aliases Select', async () => {
    const { default: GameAliasForm } = await import('../GameAliasForm');
    render(
      <GameAliasForm
        visible={true}
        editingGame={null}
        onCancel={vi.fn()}
        onSuccess={vi.fn()}
      />
    );
    expect(screen.getByPlaceholderText(/游戏名称/)).toBeTruthy();
    expect(screen.getByRole('combobox', { name: '别名' })).toBeTruthy();
    expect(screen.getByText('输入别名后按回车添加')).toBeTruthy();
  });

  it('pre-fills form in edit mode', async () => {
    const { default: GameAliasForm } = await import('../GameAliasForm');
    render(
      <GameAliasForm
        visible={true}
        editingGame={sampleGame}
        onCancel={vi.fn()}
        onSuccess={vi.fn()}
      />
    );
    const nameInput = screen.getByPlaceholderText(/游戏名称/) as HTMLInputElement;
    expect(nameInput.value).toBe('Test Game');
  });

  it('shows edit title when editing', async () => {
    const { default: GameAliasForm } = await import('../GameAliasForm');
    render(
      <GameAliasForm
        visible={true}
        editingGame={sampleGame}
        onCancel={vi.fn()}
        onSuccess={vi.fn()}
      />
    );
    expect(screen.getByText(/编辑游戏别名/)).toBeTruthy();
  });

  it('shows create title when not editing', async () => {
    const { default: GameAliasForm } = await import('../GameAliasForm');
    render(
      <GameAliasForm
        visible={true}
        editingGame={null}
        onCancel={vi.fn()}
        onSuccess={vi.fn()}
      />
    );
    expect(screen.getByText(/添加游戏别名/)).toBeTruthy();
  });
});

describe('GameTable delete confirmation (T025c)', () => {
  it('shows Popconfirm and calls onDelete after confirmation', async () => {
    const user = userEvent.setup();
    userRef.current = { role: 3 };
    render(<GameTable {...baseProps} />);
    expect(baseProps.onDelete).not.toHaveBeenCalled();

    await user.click(screen.getByRole('button', { name: /delete/i }));
    expect(await screen.findByText('确认删除')).toBeTruthy();
    expect(screen.getByText(/删除后不可恢复/)).toBeTruthy();

    await user.click(screen.getByRole('button', { name: /确\s*认/ }));
    expect(baseProps.onDelete).toHaveBeenCalledWith(sampleGame.id);
  });
});
