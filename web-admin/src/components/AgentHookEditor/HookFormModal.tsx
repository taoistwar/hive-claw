import React, { useEffect } from 'react';
import { Modal, Form, Input, Select, Switch, InputNumber, Button, Space, message } from 'antd';
import {
  createHook,
  updateHook,
  type AgentHook,
  type TriggerPoint,
  type ActionType,
  TRIGGER_POINT_LABELS,
  ACTION_TYPE_LABELS,
  type CreateHookRequest,
  type UpdateHookRequest,
} from '../../services/agentHook';
import { listWorkflows, type WorkflowMeta } from '../../services/workflow';
import { listFunctions, type FunctionItem } from '../../services/function';

const { TextArea } = Input;

interface Props {
  open: boolean;
  agentId: number;
  editingHook: AgentHook | null;
  onClose: () => void;
  onSave: (values?: Record<string, unknown>) => void;
}

const TRIGGER_OPTIONS: { value: TriggerPoint; label: string }[] = Object.entries(
  TRIGGER_POINT_LABELS,
).map(([value, label]) => ({ value: value as TriggerPoint, label }));

const ACTION_OPTIONS: { value: ActionType; label: string }[] = Object.entries(
  ACTION_TYPE_LABELS,
).map(([value, label]) => ({ value: value as ActionType, label }));

const HookFormModal: React.FC<Props> = ({ open, agentId, editingHook, onClose, onSave }) => {
  const [form] = Form.useForm();
  const [saving, setSaving] = React.useState(false);
  const [workflows, setWorkflows] = React.useState<WorkflowMeta[]>([]);
  const [functions, setFunctions] = React.useState<FunctionItem[]>([]);
  const [loadingOptions, setLoadingOptions] = React.useState(false);
  const actionType = Form.useWatch('action_type', form);

  const loadOptions = async () => {
    setLoadingOptions(true);
    try {
      const [wfResp, fnResp] = await Promise.all([
        listWorkflows({ limit: 200 }),
        listFunctions({ offset: 0, limit: 200 }),
      ]);
      setWorkflows(wfResp.items);
      setFunctions(fnResp.items);
    } catch {
      message.error('加载选项失败');
    } finally {
      setLoadingOptions(false);
    }
  };

  useEffect(() => {
    if (open) {
      loadOptions();
      if (editingHook) {
        form.setFieldsValue({
          name: editingHook.name,
          description: editingHook.description,
          trigger_point: editingHook.trigger_point,
          action_type: editingHook.action_type,
          function_id: (editingHook.action_params as Record<string, unknown>)?.function_id,
          workflow_id: (editingHook.action_params as Record<string, unknown>)?.workflow_id,
          webhook_url: (editingHook.action_params as Record<string, unknown>)?.webhook_url,
          args_json: (editingHook.action_params as Record<string, unknown>)?.args
            ? JSON.stringify((editingHook.action_params as Record<string, unknown>).args, null, 2)
            : '',
          enabled: editingHook.enabled,
          sort_order: editingHook.sort_order,
          blocking_mode: editingHook.blocking_mode,
          timeout_ms: editingHook.timeout_ms,
        });
      } else {
        form.resetFields();
        form.setFieldsValue({
          enabled: true,
          sort_order: 0,
          blocking_mode: false,
          timeout_ms: 10000,
        });
      }
    }
  }, [open, editingHook, form]);

  const buildActionParams = (values: Record<string, unknown>): Record<string, unknown> => {
    const at = values.action_type as ActionType;
    const params: Record<string, unknown> = {};

    if (at === 'call_function') {
      params.function_id = values.function_id;
      if (values.args_json) {
        try {
          params.args = JSON.parse(values.args_json as string);
        } catch {
          message.error('入参 JSON 格式错误');
          throw new Error('Invalid JSON');
        }
      }
    } else if (at === 'call_workflow') {
      params.workflow_id = values.workflow_id;
      if (values.args_json) {
        try {
          params.args = JSON.parse(values.args_json as string);
        } catch {
          message.error('入参 JSON 格式错误');
          throw new Error('Invalid JSON');
        }
      }
    } else if (at === 'http_webhook') {
      params.webhook_url = values.webhook_url;
      params.timeout_ms = values.timeout_ms;
    }

    return params;
  };

  const handleSubmit = async () => {
    try {
      const values = await form.validateFields();
      const actionParams = buildActionParams(values);
      setSaving(true);

      if (editingHook) {
        const meta: UpdateHookRequest = {
          name: values.name,
          description: values.description ?? null,
          trigger_point: values.trigger_point,
          action_type: values.action_type,
          action_params: actionParams,
          enabled: values.enabled,
          sort_order: values.sort_order,
          blocking_mode: values.blocking_mode,
          timeout_ms: values.timeout_ms,
          updated_at: editingHook.updated_at,
        };
        await updateHook(agentId, editingHook.id, meta);
        message.success('Hook 已更新');
      } else {
        const meta: CreateHookRequest = {
          name: values.name,
          description: values.description,
          trigger_point: values.trigger_point,
          action_type: values.action_type,
          action_params: actionParams,
          enabled: values.enabled,
          sort_order: values.sort_order,
          blocking_mode: values.blocking_mode,
          timeout_ms: values.timeout_ms,
        };
        await createHook(agentId, meta);
        message.success('Hook 已创建');
      }
      onSave();
    } catch (err) {
      if (err instanceof Error && err.message !== 'Invalid JSON') {
        message.error('保存失败');
      }
    } finally {
      setSaving(false);
    }
  };

  return (
    <Modal
      title={editingHook ? '编辑 Hook' : '新建 Hook'}
      open={open}
      onCancel={onClose}
      onOk={handleSubmit}
      confirmLoading={saving}
      width={560}
      destroyOnClose
    >
      <Form form={form} layout="vertical" size="small">
        <Form.Item name="name" label="名称" rules={[{ required: true, message: '请输入名称' }]}>
          <Input maxLength={128} />
        </Form.Item>

        <Form.Item name="description" label="描述">
          <TextArea rows={2} maxLength={500} />
        </Form.Item>

        <Form.Item name="trigger_point" label="触发点" rules={[{ required: true }]}>
          <Select options={TRIGGER_OPTIONS} />
        </Form.Item>

        <Form.Item name="action_type" label="动作类型" rules={[{ required: true }]}>
          <Select options={ACTION_OPTIONS} />
        </Form.Item>

        {actionType === 'call_function' && (
          <>
            <Form.Item name="function_id" label="Function" rules={[{ required: true }]}>
              <Select
                placeholder="选择 Function"
                loading={loadingOptions}
                options={functions.map((f) => ({
                  value: f.id,
                  label: `${f.name} (${f.identifier})`,
                }))}
              />
            </Form.Item>
            <Form.Item name="args_json" label="入参 (JSON)">
              <TextArea rows={3} placeholder='{"key": "value"}' />
            </Form.Item>
          </>
        )}

        {actionType === 'call_workflow' && (
          <>
            <Form.Item name="workflow_id" label="Workflow" rules={[{ required: true }]}>
              <Select
                placeholder="选择 Workflow"
                loading={loadingOptions}
                options={workflows.map((w) => ({
                  value: w.id,
                  label: `${w.name} (${w.identifier})`,
                }))}
              />
            </Form.Item>
            <Form.Item name="args_json" label="入参 (JSON)">
              <TextArea rows={3} placeholder='{"key": "value"}' />
            </Form.Item>
          </>
        )}

        {actionType === 'http_webhook' && (
          <Form.Item
            name="webhook_url"
            label="Webhook URL"
            rules={[
              { required: true, message: '请输入 URL' },
              { type: 'url', message: '请输入有效的 HTTPS URL' },
              {
                pattern: /^https:\/\//,
                message: '仅支持 HTTPS 协议',
              },
            ]}
          >
            <Input placeholder="https://example.com/webhook" />
          </Form.Item>
        )}

        <Space style={{ width: '100%' }}>
          <Form.Item name="enabled" label="启用" valuePropName="checked">
            <Switch />
          </Form.Item>
          <Form.Item name="blocking_mode" label="阻塞模式" valuePropName="checked">
            <Switch />
          </Form.Item>
          <Form.Item name="timeout_ms" label="超时(ms)">
            <InputNumber min={1000} max={60000} style={{ width: 100 }} />
          </Form.Item>
          <Form.Item name="sort_order" label="排序">
            <InputNumber min={0} max={100} style={{ width: 60 }} />
          </Form.Item>
        </Space>
      </Form>
    </Modal>
  );
};

export default HookFormModal;
