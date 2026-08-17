/// <reference types="vitest/globals" />
import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import KnowledgeQueryPage from '../KnowledgeQueryPage'

const { getKnowledgeQueryDefaults, queryKnowledge } = vi.hoisted(() => ({
  getKnowledgeQueryDefaults: vi.fn(),
  queryKnowledge: vi.fn(),
}))

vi.mock('../../services/knowledgeQuery', () => ({
  getKnowledgeQueryDefaults,
  queryKnowledge,
}))

describe('KnowledgeQueryPage', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    getKnowledgeQueryDefaults.mockResolvedValue({
      page: 2,
      page_size: 6,
      similarity_threshold: 0.2,
      vector_similarity_weight: 0.3,
      top_k: 10,
      rerank_id: 'reranker-from-global',
      keyword: true,
      highlight: false,
      timeout_secs: 30,
    })
    queryKnowledge.mockResolvedValue({
      items: [],
      total: 0,
      page: 4,
      page_size: 20,
    })
  })

  it('loads global defaults and submits every editable retrieval parameter', async () => {
    const user = userEvent.setup()
    render(<KnowledgeQueryPage />)

    await waitFor(() => expect(screen.getByLabelText('页码')).toHaveValue('2'))
    expect(screen.getByLabelText('每页数量')).toHaveValue('6')
    expect(screen.getByLabelText('相似度阈值')).toHaveValue('0.20')
    expect(screen.getByLabelText('向量相似度权重')).toHaveValue('0.30')
    expect(screen.getByLabelText('Top K')).toHaveValue('10')
    expect(screen.getByLabelText('Rerank ID')).toHaveValue('reranker-from-global')
    expect(screen.getByRole('switch', { name: '关键字匹配' })).toBeChecked()
    expect(screen.getByRole('switch', { name: '返回高亮' })).not.toBeChecked()
    expect(screen.getByLabelText('请求超时（秒）')).toHaveValue('30')

    await user.type(screen.getByLabelText('问题'), '如何重置密码？')

    const pageInput = screen.getByLabelText('页码')
    await user.clear(pageInput)
    await user.type(pageInput, '4')

    const pageSizeInput = screen.getByLabelText('每页数量')
    await user.clear(pageSizeInput)
    await user.type(pageSizeInput, '20')

    const similarityInput = screen.getByLabelText('相似度阈值')
    await user.clear(similarityInput)
    await user.type(similarityInput, '0.45')

    const vectorWeightInput = screen.getByLabelText('向量相似度权重')
    await user.clear(vectorWeightInput)
    await user.type(vectorWeightInput, '0.65')

    const topKInput = screen.getByLabelText('Top K')
    await user.clear(topKInput)
    await user.type(topKInput, '25')

    const rerankInput = screen.getByLabelText('Rerank ID')
    await user.clear(rerankInput)
    await user.type(rerankInput, 'debug-reranker')

    await user.click(screen.getByRole('switch', { name: '关键字匹配' }))
    await user.click(screen.getByRole('switch', { name: '返回高亮' }))

    const timeoutInput = screen.getByLabelText('请求超时（秒）')
    await user.clear(timeoutInput)
    await user.type(timeoutInput, '45')

    await user.click(screen.getByRole('button', { name: '检索' }))

    await waitFor(() => {
      expect(queryKnowledge).toHaveBeenCalledWith({
        question: '如何重置密码？',
        page: 4,
        page_size: 20,
        similarity_threshold: 0.45,
        vector_similarity_weight: 0.65,
        top_k: 25,
        rerank_id: 'debug-reranker',
        keyword: false,
        highlight: true,
        timeout_secs: 45,
      })
    })
  })
})
