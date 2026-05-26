import { ReactNode } from 'react';
import { useAuth } from '../hooks/useAuth';
import { Result } from 'antd';

interface PermissionGuardProps {
  requiredRole: number;
  children: ReactNode;
}

const PermissionGuard: React.FC<PermissionGuardProps> = ({
  requiredRole,
  children,
}) => {
  const { user } = useAuth();

  if (!user) {
    return <Result status="403" title="无权限" subTitle="请先登录" />;
  }

  if (user.role < requiredRole) {
    return <Result status="403" title="无权限" subTitle="您的权限不足，无法访问此页面" />;
  }

  return <>{children}</>;
};

export default PermissionGuard;
