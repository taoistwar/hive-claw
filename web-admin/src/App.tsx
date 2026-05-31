import { useMemo } from 'react'
import { BrowserRouter, Routes, Route, Navigate } from 'react-router-dom'
import { ConfigProvider, theme as antdTheme } from 'antd'
import LoginPage from './pages/LoginPage'
import DashboardPage from './pages/DashboardPage'
import AdminPage from './pages/AdminPage'
import PluginPage from './pages/PluginPage'
import FunctionPage from './pages/FunctionPage'
import ToolPage from './pages/ToolPage'
import SkillPage from './pages/SkillPage'
import CategoryPage from './pages/CategoryPage'
import TagPage from './pages/TagPage'
import AgentPage from './pages/AgentPage'
import WorkflowPage from './pages/WorkflowPage'
import ChatPage from './pages/ChatPage'
import RecommendedGamePage from './pages/RecommendedGamePage'
import CapabilityPage from './pages/CapabilityPage'
import RuntimeAuditLogPage from './pages/RuntimeAuditLogPage'
import AdminAuditLogPage from './pages/AdminAuditLogPage'
import LoginRecordPage from './pages/LoginRecordPage'
import ChangePasswordPage from './pages/ChangePasswordPage'
import UserManagementPage from './pages/UserManagementPage'
import Layout from './components/Layout'
import { AuthProvider } from './hooks/useAuth'
import { ThemeProvider, useTheme } from './hooks/useTheme'

const THEME_TOKENS = {
  dark: {
    colorBgBase: '#111827',
    colorTextBase: '#f0f4f8',
    Layout: { headerBg: '#1a2035', bodyBg: '#0a0e17', siderBg: '#111827' },
    Menu: {
      itemBg: 'transparent',
      itemColor: '#8899aa',
      itemSelectedBg: 'rgba(102, 126, 234, 0.15)',
      itemSelectedColor: '#f0f4f8',
      itemHoverBg: 'rgba(255, 255, 255, 0.05)',
      itemHoverColor: '#f0f4f8',
      horizontalItemHoverBg: 'rgba(255, 255, 255, 0.05)',
      horizontalItemSelectedBg: 'rgba(102, 126, 234, 0.15)',
    },
    Card: { colorBgContainer: '#1a2035' },
    Table: {
      colorBgContainer: '#1a2035',
      headerBg: '#111827',
      headerColor: '#8899aa',
      rowHoverBg: 'rgba(102, 126, 234, 0.04)',
      borderColor: 'rgba(255, 255, 255, 0.06)',
    },
    Input: {
      colorBgBase: '#0f1524',
      activeBorderColor: '#667eea',
      hoverBorderColor: '#667eea',
      activeShadow: '0 0 0 2px rgba(102, 126, 234, 0.1)',
    },
    Select: { colorBgBase: '#0f1524', selectorBg: '#0f1524' },
    Button: { defaultBg: '#1a2035', defaultBorderColor: 'rgba(255, 255, 255, 0.06)' },
    Avatar: { colorBgContainer: '#667eea' },
    Dropdown: { colorBgElevated: '#1a2035' },
    Modal: { colorBgContainer: '#1a2035', colorBgMask: 'rgba(0, 0, 0, 0.7)' },
    Drawer: { colorBgContainer: '#1a2035', colorBgElevated: '#1a2035' },
  },
  light: {
    colorBgBase: '#ffffff',
    colorTextBase: '#1a202c',
    Layout: { headerBg: '#ffffff', bodyBg: '#f5f7fa', siderBg: '#ffffff' },
    Menu: {
      itemBg: 'transparent',
      itemColor: '#4a5568',
      itemSelectedBg: 'rgba(102, 126, 234, 0.1)',
      itemSelectedColor: '#667eea',
      itemHoverBg: 'rgba(102, 126, 234, 0.06)',
      itemHoverColor: '#667eea',
      horizontalItemHoverBg: 'rgba(102, 126, 234, 0.06)',
      horizontalItemSelectedBg: 'rgba(102, 126, 234, 0.1)',
    },
    Card: { colorBgContainer: '#ffffff' },
    Table: {
      colorBgContainer: '#ffffff',
      headerBg: '#f8f9fb',
      headerColor: '#4a5568',
      rowHoverBg: 'rgba(102, 126, 234, 0.04)',
      borderColor: 'rgba(0, 0, 0, 0.08)',
    },
    Input: {
      colorBgBase: '#f0f2f5',
      activeBorderColor: '#667eea',
      hoverBorderColor: '#667eea',
      activeShadow: '0 0 0 2px rgba(102, 126, 234, 0.1)',
    },
    Select: { colorBgBase: '#f0f2f5', selectorBg: '#f0f2f5' },
    Button: { defaultBg: '#ffffff', defaultBorderColor: 'rgba(0, 0, 0, 0.08)' },
    Avatar: { colorBgContainer: '#667eea' },
    Dropdown: { colorBgElevated: '#ffffff' },
    Modal: { colorBgContainer: '#ffffff', colorBgMask: 'rgba(0, 0, 0, 0.45)' },
    Drawer: { colorBgContainer: '#ffffff', colorBgElevated: '#ffffff' },
  },
}

function ThemeConfigProvider({ children }: { children: React.ReactNode }) {
  const { theme } = useTheme()
  const isDark = theme === 'dark'

  const config = useMemo(() => {
    const t = THEME_TOKENS[theme]
    return {
      algorithm: isDark ? antdTheme.darkAlgorithm : antdTheme.defaultAlgorithm,
      token: {
        colorPrimary: '#667eea',
        colorBgBase: t.colorBgBase,
        colorTextBase: t.colorTextBase,
        borderRadius: 8,
        fontFamily: "'DM Sans', 'Inter', system-ui, -apple-system, sans-serif",
      },
      components: {
        Layout: t.Layout,
        Menu: t.Menu,
        Card: t.Card,
        Table: t.Table,
        Input: t.Input,
        Select: t.Select,
        Button: t.Button,
        Avatar: t.Avatar,
        Dropdown: t.Dropdown,
        Modal: t.Modal,
        Drawer: t.Drawer,
      },
    }
  }, [theme, isDark])

  return <ConfigProvider theme={config}>{children}</ConfigProvider>
}

function AppRoutes() {
  return (
    <BrowserRouter>
      <Routes>
        <Route path="/login" element={<LoginPage />} />
        <Route path="/" element={<Layout />}>
          <Route index element={<DashboardPage />} />
          <Route path="admins" element={<AdminPage />} />
          <Route path="plugins" element={<PluginPage />} />
          <Route path="capabilities" element={<CapabilityPage />} />
          <Route path="functions" element={<FunctionPage />} />
          <Route path="tools" element={<ToolPage />} />
          <Route path="skills" element={<SkillPage />} />
          <Route path="categories" element={<CategoryPage />} />
          <Route path="tags" element={<TagPage />} />
          <Route path="agents" element={<AgentPage />} />
          <Route path="workflows" element={<WorkflowPage />} />
          <Route path="chat" element={<ChatPage />} />
          <Route path="recommended-games" element={<RecommendedGamePage />} />
          <Route path="users" element={<UserManagementPage />} />
          <Route path="admin-audit-logs" element={<AdminAuditLogPage />} />
          <Route path="runtime-audit-logs" element={<RuntimeAuditLogPage />} />
          <Route path="login-records" element={<LoginRecordPage />} />
          <Route path="settings/change-password" element={<ChangePasswordPage />} />
        </Route>
        <Route path="*" element={<Navigate to="/" replace />} />
      </Routes>
    </BrowserRouter>
  )
}

function App() {
  return (
    <ThemeProvider>
      <ThemeConfigProvider>
        <AuthProvider>
          <AppRoutes />
        </AuthProvider>
      </ThemeConfigProvider>
    </ThemeProvider>
  )
}

export default App