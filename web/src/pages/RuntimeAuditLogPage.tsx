import { Typography } from 'antd';
const { Title } = Typography;
import { useRuntimeAuditLogs } from '../hooks/useRuntimeAuditLogs';
import RuntimeAuditLogTable from '../components/RuntimeAuditLogTable';

const RuntimeAuditLogPage: React.FC = () => {
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
  } = useRuntimeAuditLogs();

  const handlePageChange = (page: number, pageSize: number) => {
    setPagination({ ...pagination, current: page, pageSize });
    fetchAuditLogs(page);
  };

  return (
    <div>
      <Title level={2}>Agent 审计日志</Title>
      <RuntimeAuditLogTable
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

export default RuntimeAuditLogPage;
