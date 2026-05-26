import { Typography, Button } from 'antd';
import { PlusOutlined } from '@ant-design/icons';
import { useEffect } from 'react';
import { useAdmin } from '../hooks/useAdmin';
import AdminTable from '../components/AdminTable';
import AdminForm from '../components/AdminForm';

const { Title } = Typography;

const AdminPage: React.FC = () => {
  const {
    admins,
    loading,
    pagination,
    modalVisible,
    editingAdmin,
    fetchAdmins,
    handleCreate,
    handleUpdate,
    handleDelete,
    handleToggleStatus,
    openCreateModal,
    openEditModal,
    closeModal,
    setPagination,
  } = useAdmin();

  useEffect(() => {
    fetchAdmins();
  }, []);

  const handlePaginationChange = (page: number, pageSize: number) => {
    setPagination((prev) => ({ ...prev, current: page, pageSize }));
    fetchAdmins(page);
  };

  return (
    <div>
      <div
        style={{
          display: 'flex',
          justifyContent: 'space-between',
          alignItems: 'center',
          marginBottom: 24,
        }}
      >
        <Title level={4} style={{ margin: 0 }}>
          管理员管理
        </Title>
        <Button type="primary" icon={<PlusOutlined />} onClick={openCreateModal}>
          添加管理员
        </Button>
      </div>

      <AdminTable
        admins={admins}
        loading={loading}
        pagination={pagination}
        onPaginationChange={handlePaginationChange}
        onEdit={openEditModal}
        onDelete={handleDelete}
        onToggleStatus={handleToggleStatus}
      />

      <AdminForm
        visible={modalVisible}
        editingAdmin={editingAdmin}
        onCancel={closeModal}
        onCreate={handleCreate}
        onUpdate={handleUpdate}
      />
    </div>
  );
};

export default AdminPage;
