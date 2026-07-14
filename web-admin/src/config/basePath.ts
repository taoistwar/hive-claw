const viteBase = import.meta.env.BASE_URL.replace(/\/+$/, '')
const configuredBase = viteBase && viteBase !== '/' ? viteBase : '/web-admin'

export const WEB_ADMIN_BASE_PATH = configuredBase || '/'

export function webAdminUrl(path: string): string {
  const normalizedPath = path.startsWith('/') ? path : `/${path}`
  return WEB_ADMIN_BASE_PATH === '/'
    ? normalizedPath
    : `${WEB_ADMIN_BASE_PATH}${normalizedPath}`
}
