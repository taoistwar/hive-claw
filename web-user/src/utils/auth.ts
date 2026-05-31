export const setToken = (token: string) => {
  localStorage.setItem('user_auth_token', token);
};

export const getToken = (): string | null => {
  return localStorage.getItem('user_auth_token');
};

export const removeToken = () => {
  localStorage.removeItem('user_auth_token');
};
