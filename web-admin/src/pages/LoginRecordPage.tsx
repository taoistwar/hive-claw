import { Typography } from 'antd';
import { useEffect } from 'react';
import { useLoginRecords } from '../hooks/useLoginRecords';
import LoginRecordTable from '../components/LoginRecordTable';

const { Title } = Typography;

const LoginRecordPage: React.FC = () => {
  const {
    loginRecords,
    loading,
    pagination,
    searchParams,
    selectedRecord,
    detailModalVisible,
    fetchLoginRecords,
    setPagination,
    setSearchParams,
    handleSearch,
    handleReset,
    viewDetail,
    closeDetailModal,
  } = useLoginRecords();

  useEffect(() => {
    fetchLoginRecords();
  }, []);

  const handlePaginationChange = (page: number, pageSize: number) => {
    setPagination((prev) => ({ ...prev, current: page, pageSize }));
    fetchLoginRecords(page);
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
          登录日志
        </Title>
      </div>

      <LoginRecordTable
        loginRecords={loginRecords}
        loading={loading}
        pagination={pagination}
        onPaginationChange={handlePaginationChange}
        onViewDetail={viewDetail}
        searchParams={searchParams}
        onSearchParamsChange={setSearchParams}
        onSearch={handleSearch}
        onReset={handleReset}
        selectedRecord={selectedRecord}
        detailModalVisible={detailModalVisible}
        onCloseDetailModal={closeDetailModal}
      />
    </div>
  );
};

export default LoginRecordPage;
