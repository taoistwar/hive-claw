import { createContext, useContext, useState, useEffect, ReactNode } from 'react';
import { login as apiLogin, logout as apiLogout, getCurrentUser } from '../services/auth';
import { setToken, getToken, removeToken } from '../utils/auth';

interface User {
  id: number;
  phone: string;
  created_at: string;
  updated_at: string;
}

interface AuthContextType {
  user: User | null;
  loginUser: (phone: string, password: string) => Promise<void>;
  logout: () => void;
  isAuthenticated: boolean;
}

const AuthContext = createContext<AuthContextType | null>(null);

export const AuthProvider = ({ children }: { children: ReactNode }) => {
  const [user, setUser] = useState<User | null>(null);

  useEffect(() => {
    const token = getToken();
    if (token) {
      getCurrentUser()
        .then((userData) => setUser(userData))
        .catch(() => {
          removeToken();
          setUser(null);
        });
    }
  }, []);

  const loginUser = async (phone: string, password: string) => {
    const response = await apiLogin({ phone, password });
    setToken(response.token);
    setUser(response.user);
  };

  const logout = async () => {
    try {
      await apiLogout();
    } finally {
      removeToken();
      setUser(null);
    }
  };

  return (
    <AuthContext.Provider value={{ 
      user, 
      loginUser, 
      logout, 
      isAuthenticated: !!user 
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
