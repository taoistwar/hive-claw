import apiClient from './api';

export interface LoginRequest {
  phone: string;
  password: string;
}

export interface RegisterRequest {
  phone: string;
  password: string;
}

export interface AdminInfo {
  id: number;
  phone: string;
  nickname: string;
  role: number;
  status: number;
}

export interface UserInfo {
  id: number;
  phone: string;
  created_at: string;
  updated_at: string;
}

export interface AdminLoginResponse {
  token: string;
  admin: AdminInfo;
}

export interface UserLoginResponse {
  token: string;
  user: UserInfo;
}

export const login = async (data: LoginRequest): Promise<AdminLoginResponse> => {
  const response = await apiClient.post('/auth/login', data);
  return response.data;
};

export const registerUser = async (data: RegisterRequest): Promise<UserLoginResponse> => {
  const response = await apiClient.post('/users/register', data);
  return response.data;
};

export const loginUser = async (data: LoginRequest): Promise<UserLoginResponse> => {
  const response = await apiClient.post('/users/login', data);
  return response.data;
};

export const logout = async (): Promise<void> => {
  await apiClient.post('/auth/logout');
};

export const getCurrentAdmin = async (): Promise<AdminInfo> => {
  const response = await apiClient.get('/auth/me');
  return response.data;
};

export const getCurrentUser = async (): Promise<UserInfo> => {
  const response = await apiClient.get('/users/me');
  return response.data;
};

export interface ChangePasswordRequest {
  old_password: string;
  new_password: string;
}

export const changePassword = async (data: ChangePasswordRequest): Promise<void> => {
  await apiClient.post('/auth/change-password', data);
};
