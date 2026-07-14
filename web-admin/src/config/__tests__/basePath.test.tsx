import { fireEvent, render, screen } from '@testing-library/react'
import { BrowserRouter, useNavigate } from 'react-router-dom'
import { afterEach, describe, expect, it } from 'vitest'
import { WEB_ADMIN_BASE_PATH, webAdminUrl } from '../basePath'

function NavigationProbe() {
  const navigate = useNavigate()
  return <button onClick={() => navigate('/users')}>users</button>
}

describe('web-admin base path', () => {
  afterEach(() => {
    window.history.replaceState({}, '', '/web-admin/')
  })

  it('keeps client-side navigation under /web-admin', () => {
    window.history.replaceState({}, '', '/web-admin/')
    render(
      <BrowserRouter basename={WEB_ADMIN_BASE_PATH}>
        <NavigationProbe />
      </BrowserRouter>,
    )

    fireEvent.click(screen.getByRole('button', { name: 'users' }))

    expect(window.location.pathname).toBe('/web-admin/users')
  })

  it('builds full browser redirects under /web-admin', () => {
    expect(webAdminUrl('/login')).toBe('/web-admin/login')
  })
})
