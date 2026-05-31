import apiClient from './api';

export interface LoginRequest {
  phone: string;
  password: string;
}

export interface User {
  id: number;
  phone: string;
  created_at: string;
  updated_at: string;
}

export interface AuthResponse {
  token: string;
  user: User;
}

export const login = (data: LoginRequest) => {
  return apiClient.post<AuthResponse>('/users/login', data).then((res) => res.data);
};

export const logout = () => {
  return apiClient.post('/users/logout').then((res) => res.data);
};

export const getCurrentUser = () => {
  return apiClient.get<User>('/users/me').then((res) => res.data);
};

export interface ChangePasswordRequest {
  old_password: string;
  new_password: string;
}

export const changePassword = (data: ChangePasswordRequest) => {
  return apiClient.post('/users/change-password', data).then((res) => res.data);
};
