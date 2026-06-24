//! Admin API client for sensitive word CRUD.
//! See `specs/010-sensitive-word-filter/contracts/api.md`.

import apiClient from './api';

export interface SensitiveWord {
    id: number;
    word: string;
    match_mode: 'exact' | 'regex';
    enabled: boolean;
    created_at: string;
    updated_at: string;
}

export interface SensitiveWordListResponse {
    total: number;
    words: SensitiveWord[];
}

export interface CreateSensitiveWordRequest {
    word: string;
    match_mode?: 'exact' | 'regex';
}

export interface UpdateSensitiveWordRequest {
    word?: string;
    match_mode?: 'exact' | 'regex';
    enabled?: boolean;
}

export async function listSensitiveWords(params?: {
    page?: number;
    page_size?: number;
    search?: string;
}): Promise<SensitiveWordListResponse> {
    const resp = await apiClient.get<SensitiveWordListResponse>('/sensitive-words', { params });
    return resp.data;
}

export async function createSensitiveWord(
    data: CreateSensitiveWordRequest,
): Promise<SensitiveWord> {
    const resp = await apiClient.post<SensitiveWord>('/sensitive-words', data);
    return resp.data;
}

export async function updateSensitiveWord(
    id: number,
    data: UpdateSensitiveWordRequest,
): Promise<SensitiveWord> {
    const resp = await apiClient.put<SensitiveWord>(`/sensitive-words/${id}`, data);
    return resp.data;
}

export async function deleteSensitiveWord(id: number): Promise<{ deleted: boolean }> {
    const resp = await apiClient.delete<{ deleted: boolean }>(`/sensitive-words/${id}`);
    return resp.data;
}
