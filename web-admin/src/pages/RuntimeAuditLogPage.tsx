import { Alert, Typography } from 'antd'
import RuntimeAuditLogTable from '../components/RuntimeAuditLogTable'
import { useRuntimeAuditLogs } from '../hooks/useRuntimeAuditLogs'

const { Title } = Typography

const RuntimeAuditLogPage: React.FC = () => {
  const audit = useRuntimeAuditLogs()

  return (
    <div>
      <Title level={2}>运行时审计日志</Title>
      {audit.error && (
        <Alert type="error" showIcon message={audit.error} style={{ marginBottom: 16 }} />
      )}
      <RuntimeAuditLogTable
        auditLogs={audit.auditLogs}
        loading={audit.loading}
        pagination={audit.pagination}
        onPaginationChange={audit.handlePageChange}
        onViewDetail={audit.handleViewDetail}
        onSearch={audit.handleSearch}
        onReset={audit.handleReset}
        selectedLog={audit.selectedLog}
        detailModalVisible={audit.showDetail}
        onCloseDetailModal={audit.handleCloseDetail}
      />
    </div>
  )
}

export default RuntimeAuditLogPage
