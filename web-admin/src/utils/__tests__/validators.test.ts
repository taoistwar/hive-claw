/// <reference types="vitest/globals" />

import { describe, expect, it } from 'vitest'

import { validatePassword } from '../validators'

describe('validatePassword', () => {
  it('counts Unicode code points instead of UTF-16 code units', () => {
    expect(validatePassword('a1😀😀😀')).toBe(false)
    expect(validatePassword('a1😀😀😀😀')).toBe(true)
    expect(validatePassword(`a1${'😀'.repeat(18)}`)).toBe(true)
    expect(validatePassword(`a1${'😀'.repeat(19)}`)).toBe(false)
  })

  it('requires at least one ASCII letter and one ASCII digit', () => {
    expect(validatePassword('123456')).toBe(false)
    expect(validatePassword('abcdef')).toBe(false)
    expect(validatePassword('１２３abc')).toBe(false)
    expect(validatePassword('a1密码安全')).toBe(true)
  })

  it('allows non-ASCII characters when the shared strength policy is met', () => {
    expect(validatePassword('a1中文世界')).toBe(true)
    expect(validatePassword('Z9🔐安全密钥')).toBe(true)
  })
})
