import { Typography } from 'antd';
const { Title } = Typography;
import { useAdminAuditLogs } from '../hooks/useAdminAuditLogs';
import AdminAuditLogTable from '../components/AdminAuditLogTable';

const AdminAuditLogPage: React.FC = () => {
  const {
    auditLogs,
    loading,
    pagination,
    searchParams,
    selectedLog,
    showDetail,
    fetchAuditLogs,
    handleSearch,
    handleReset,
    handleViewDetail,
    handleCloseDetail,
    setPagination,
  } = useAdminAuditLogs();

  const handlePageChange = (page: number, pageSize: number) => {
    setPagination({ ...pagination, current: page, pageSize });
    fetchAuditLogs(page);
  };

  return (
    <div>
      <Title level={2}>管理审计日志</Title>
      <AdminAuditLogTable
        auditLogs={auditLogs}
        loading={loading}
        pagination={pagination}
        onPaginationChange={handlePageChange}
        onViewDetail={handleViewDetail}
        searchParams={searchParams}
        onSearchParamsChange={() => {}}
        onSearch={handleSearch}
        onReset={handleReset}
        selectedLog={selectedLog}
        detailModalVisible={showDetail}
        onCloseDetailModal={handleCloseDetail}
      />
    </div>
  );
};

export default AdminAuditLogPage;
