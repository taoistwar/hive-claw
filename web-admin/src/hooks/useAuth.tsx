import { createContext, useContext, useState, useEffect, ReactNode } from 'react';
import { login as apiLogin, loginUser as apiLoginUser, logout as apiLogout, getCurrentAdmin, getCurrentUser } from '../services/auth';
import { setToken, getToken, removeToken } from '../utils/auth';

interface Admin {
  id: number;
  phone: string;
  nickname: string;
  role: number;
  status: number;
}

interface User {
  id: number;
  phone: string;
  created_at: string;
  updated_at: string;
}

interface AuthContextType {
  admin: Admin | null;
  user: User | null;
  loginAdmin: (phone: string, password: string) => Promise<void>;
  loginUser: (phone: string, password: string) => Promise<void>;
  logout: () => void;
  isAuthenticated: boolean;
  isAdmin: boolean;
  isUser: boolean;
}

const AuthContext = createContext<AuthContextType | null>(null);

export const AuthProvider = ({ children }: { children: ReactNode }) => {
  const [admin, setAdmin] = useState<Admin | null>(null);
  const [user, setUser] = useState<User | null>(null);

  useEffect(() => {
    const token = getToken();
    if (token) {
      // Try to fetch admin first, then user
      getCurrentAdmin()
        .then((adminData) => setAdmin(adminData))
        .catch(() => {
          getCurrentUser()
            .then((userData) => setUser(userData))
            .catch(() => {
              removeToken();
              setAdmin(null);
              setUser(null);
            });
        });
    }
  }, []);

  const loginAdmin = async (phone: string, password: string) => {
    const response = await apiLogin({ phone, password });
    setToken(response.token);
    setAdmin(response.admin);
    setUser(null);
  };

  const loginUser = async (phone: string, password: string) => {
    const response = await apiLoginUser({ phone, password });
    setToken(response.token);
    setUser(response.user);
    setAdmin(null);
  };

  const logout = async () => {
    try {
      await apiLogout();
    } finally {
      removeToken();
      setAdmin(null);
      setUser(null);
    }
  };

  return (
    <AuthContext.Provider value={{ 
      admin, 
      user, 
      loginAdmin, 
      loginUser, 
      logout, 
      isAuthenticated: !!admin || !!user, 
      isAdmin: !!admin,
      isUser: !!user 
    }}>
      {children}
    </AuthContext.Provider>
  );
};

export const useAuth = () => {
  const context = useContext(AuthContext);
  if (!context) {
    throw new Error('useAuth must be used within an AuthProvider');
  }
  return context;
};
