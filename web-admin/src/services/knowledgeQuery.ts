import apiClient from './api'

export interface KnowledgeQueryItem {
  id: string
  content: string
  content_ltks?: string | null
  dataset_id?: string | null
  document_id?: string | null
  document_keyword?: string | null
  highlight?: string | null
  image_id?: string | null
  important_keywords?: string[] | null
  positions?: unknown[] | null
  similarity?: number | null
  term_similarity?: number | null
  vector_similarity?: number | null
}

export interface KnowledgeQueryResponse {
  items: KnowledgeQueryItem[]
  total: number
  page: number
  page_size: number
  has_knowledge: boolean
}

export interface KnowledgeQueryDefaults {
  page: number
  page_size: number
  similarity_threshold: number
  vector_similarity_weight: number
  top_k: number
  rerank_id?: string | null
  keyword: boolean
  highlight: boolean
  timeout_secs: number
}

export interface KnowledgeQueryParams {
  question: string
  page?: number
  page_size?: number
  similarity_threshold?: number
  vector_similarity_weight?: number
  top_k?: number
  rerank_id?: string
  keyword?: boolean
  highlight?: boolean
  timeout_secs?: number
}

export const getKnowledgeQueryDefaults = async (): Promise<KnowledgeQueryDefaults> => {
  const response = await apiClient.get('/knowledge-query/defaults')
  return response.data
}

export const queryKnowledge = async (
  params: KnowledgeQueryParams
): Promise<KnowledgeQueryResponse> => {
  const response = await apiClient.get('/knowledge-query', {
    params,
    timeout: params.timeout_secs ? (params.timeout_secs + 5) * 1000 : undefined,
  })
  return response.data
}
