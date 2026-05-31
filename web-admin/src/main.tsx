import React from 'react'
import ReactDOM from 'react-dom/client'
import App from './App.tsx'
import './index.css'
// 004 Agent Runtime — reactflow DAG 样式（全局；Monaco 走 @monaco-editor/react，无需 worker 手工注册）
import 'reactflow/dist/style.css'

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
)
