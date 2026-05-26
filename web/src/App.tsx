import { BrowserRouter, Routes, Route, Navigate } from 'react-router-dom'
import { ConfigProvider } from 'antd'
import LoginPage from './pages/LoginPage'
import DashboardPage from './pages/DashboardPage'
import AdminPage from './pages/AdminPage'
import PluginPage from './pages/PluginPage'
import FunctionPage from './pages/FunctionPage'
import ToolPage from './pages/ToolPage'
import SkillPage from './pages/SkillPage'
import Layout from './components/Layout'
import { AuthProvider } from './hooks/useAuth'

function App() {
  return (
    <ConfigProvider>
      <AuthProvider>
        <BrowserRouter>
          <Routes>
            <Route path="/login" element={<LoginPage />} />
            <Route path="/" element={<Layout />}>
              <Route index element={<DashboardPage />} />
              <Route path="admins" element={<AdminPage />} />
              <Route path="plugins" element={<PluginPage />} />
              <Route path="functions" element={<FunctionPage />} />
              <Route path="tools" element={<ToolPage />} />
              <Route path="skills" element={<SkillPage />} />
            </Route>
            <Route path="*" element={<Navigate to="/" replace />} />
          </Routes>
        </BrowserRouter>
      </AuthProvider>
    </ConfigProvider>
  )
}

export default App
