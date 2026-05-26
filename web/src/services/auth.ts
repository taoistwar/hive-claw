import apiClient from './api';

export interface LoginRequest {
  phone: string;
  password: string;
}

export interface LoginResponse {
  token: string;
  admin: {
    id: number;
    phone: string;
    nickname: string;
    role: number;
    status: number;
  };
}

export const login = async (data: LoginRequest): Promise<LoginResponse> => {
  const response = await apiClient.post('/auth/login', data);
  return response.data;
};

export const logout = async (): Promise<void> => {
  await apiClient.post('/auth/logout');
};

export const getCurrentUser = async (): Promise<LoginResponse['admin']> => {
  const response = await apiClient.get('/auth/me');
  return response.data;
};
