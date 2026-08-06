import apiClient from './api';

export interface KnowledgeQueryItem {
  id: string;
  content: string;
  content_ltks?: string | null;
  dataset_id?: string | null;
  document_id?: string | null;
  document_keyword?: string | null;
  highlight?: string | null;
  image_id?: string | null;
  important_keywords?: string[] | null;
  positions?: unknown[] | null;
  similarity?: number | null;
  term_similarity?: number | null;
  vector_similarity?: number | null;
}

export interface KnowledgeQueryResponse {
  items: KnowledgeQueryItem[];
  total: number;
  page: number;
  page_size: number;
}

export interface KnowledgeQueryParams {
  question: string;
  page?: number;
  page_size?: number;
}

export const queryKnowledge = async (
  params: KnowledgeQueryParams,
): Promise<KnowledgeQueryResponse> => {
  const response = await apiClient.get('/knowledge-query', {
    params: {
      question: params.question,
      page: params.page,
      page_size: params.page_size,
    },
  });
  return response.data;
};
