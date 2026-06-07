//! Sensitive word management page (010-sensitive-word-filter US3)
//! Admin CRUD for sensitive word entries with instant cache refresh.

import { useState, useEffect, useCallback } from 'react';
import {
    Button, Form, Input, Modal, Select, Space, Switch, Table,
    Typography, message, Popconfirm, Tag,
} from 'antd';
import { PlusOutlined, EditOutlined, DeleteOutlined } from '@ant-design/icons';
import type { ColumnsType } from 'antd/es/table';
import {
    listSensitiveWords, createSensitiveWord, updateSensitiveWord, deleteSensitiveWord,
    type SensitiveWord, type CreateSensitiveWordRequest, type UpdateSensitiveWordRequest,
} from '../services/sensitiveWord';

const { Text } = Typography;
const { Search } = Input;

export default function SensitiveWordPage() {
    const [words, setWords] = useState<SensitiveWord[]>([]);
    const [total, setTotal] = useState(0);
    const [loading, setLoading] = useState(false);
    const [page, setPage] = useState(1);
    const [search, setSearch] = useState('');
    const [modalOpen, setModalOpen] = useState(false);
    const [editing, setEditing] = useState<SensitiveWord | null>(null);
    const [form] = Form.useForm();

    const fetchList = useCallback(async () => {
        setLoading(true);
        try {
            const res = await listSensitiveWords({ page, page_size: 20, search: search || undefined });
            setWords(res.words);
            setTotal(res.total);
        } catch {
            message.error('加载敏感词列表失败');
        } finally {
            setLoading(false);
        }
    }, [page, search]);

    useEffect(() => { fetchList(); }, [fetchList]);

    const handleSave = async () => {
        try {
            const values = await form.validateFields();
            const req: CreateSensitiveWordRequest = {
                word: values.word,
                match_mode: values.match_mode || 'exact',
            };
            if (editing) {
                const upd: UpdateSensitiveWordRequest = {
                    word: values.word,
                    match_mode: values.match_mode,
                    enabled: values.enabled,
                };
                await updateSensitiveWord(editing.id, upd);
                message.success('敏感词已更新（缓存已刷新）');
            } else {
                await createSensitiveWord(req);
                message.success('敏感词已添加（缓存已刷新）');
            }
            setModalOpen(false);
            setEditing(null);
            form.resetFields();
            await fetchList();
        } catch (e: unknown) {
            if (e && typeof e === 'object' && 'errorFields' in e) return;
            message.error(`操作失败: ${(e as Error).message}`);
        }
    };

    const handleDelete = async (id: number) => {
        try {
            await deleteSensitiveWord(id);
            message.success('已删除（缓存已刷新）');
            await fetchList();
        } catch {
            message.error('删除失败');
        }
    };

    const openEdit = (record: SensitiveWord) => {
        setEditing(record);
        form.setFieldsValue({
            word: record.word,
            match_mode: record.match_mode,
            enabled: record.enabled,
        });
        setModalOpen(true);
    };

    const columns: ColumnsType<SensitiveWord> = [
        { title: 'ID', dataIndex: 'id', width: 80 },
        {
            title: '敏感词', dataIndex: 'word', ellipsis: true,
            render: (text: string) => <Text code>{text}</Text>,
        },
        {
            title: '匹配模式', dataIndex: 'match_mode', width: 100,
            render: (mode: string) => (
                <Tag color={mode === 'regex' ? 'blue' : 'green'}>{mode}</Tag>
            ),
        },
        {
            title: '启用', dataIndex: 'enabled', width: 80,
            render: (enabled: boolean) => <Switch checked={enabled} disabled size="small" />,
        },
        { title: '更新时间', dataIndex: 'updated_at', width: 180 },
        {
            title: '操作', width: 120,
            render: (_: unknown, record: SensitiveWord) => (
                <Space>
                    <Button icon={<EditOutlined />} size="small" onClick={() => openEdit(record)} />
                    <Popconfirm title="确定删除？" onConfirm={() => handleDelete(record.id)}>
                        <Button icon={<DeleteOutlined />} size="small" danger />
                    </Popconfirm>
                </Space>
            ),
        },
    ];

    return (
        <div>
            <Space style={{ marginBottom: 16 }}>
                <Button type="primary" icon={<PlusOutlined />} onClick={() => {
                    setEditing(null);
                    form.resetFields();
                    setModalOpen(true);
                }}>添加敏感词</Button>
                <Search
                    placeholder="搜索敏感词"
                    allowClear
                    onSearch={(val) => { setSearch(val); setPage(1); }}
                    style={{ width: 300 }}
                />
            </Space>

            <Table
                rowKey="id"
                columns={columns}
                dataSource={words}
                loading={loading}
                pagination={{ total, current: page, onChange: setPage, pageSize: 20, showTotal: (t) => `共 ${t} 条` }}
                size="small"
            />

            <Modal
                title={editing ? '编辑敏感词' : '添加敏感词'}
                open={modalOpen}
                onOk={handleSave}
                onCancel={() => { setModalOpen(false); setEditing(null); form.resetFields(); }}
                destroyOnClose
            >
                <Form form={form} layout="vertical" initialValues={{ match_mode: 'exact', enabled: true }}>
                    <Form.Item name="word" label="敏感词/正则" rules={[{ required: true, message: '请输入敏感词' }]}>
                        <Input placeholder="输入敏感词或正则表达式" maxLength={512} />
                    </Form.Item>
                    <Form.Item name="match_mode" label="匹配模式">
                        <Select>
                            <Select.Option value="exact">精确匹配（大小写不敏感）</Select.Option>
                            <Select.Option value="regex">正则表达式</Select.Option>
                        </Select>
                    </Form.Item>
                    {editing && (
                        <Form.Item name="enabled" label="启用" valuePropName="checked">
                            <Switch />
                        </Form.Item>
                    )}
                </Form>
            </Modal>
        </div>
    );
}
