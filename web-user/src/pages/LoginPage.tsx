import { Card, Typography } from 'antd';
import LoginForm from '../components/LoginForm';

const { Title } = Typography;

const LoginPage: React.FC = () => {
  return (
    <div
      style={{
        display: 'flex',
        justifyContent: 'center',
        alignItems: 'center',
        minHeight: '100vh',
        background: 'var(--bg-primary)',
        position: 'relative',
        overflow: 'hidden',
      }}
    >
      <div
        style={{
          position: 'absolute',
          inset: 0,
          background: `
            radial-gradient(circle at 20% 30%, rgba(102, 126, 234, 0.12) 0%, transparent 40%),
            radial-gradient(circle at 80% 70%, rgba(118, 75, 162, 0.12) 0%, transparent 40%),
            radial-gradient(circle at 50% 50%, rgba(99, 179, 237, 0.05) 0%, transparent 60%)
          `,
          pointerEvents: 'none',
        }}
      />

      <div
        style={{
          position: 'absolute',
          inset: 0,
          backgroundImage: `
            linear-gradient(rgba(255, 255, 255, 0.02) 1px, transparent 1px),
            linear-gradient(90deg, rgba(255, 255, 255, 0.02) 1px, transparent 1px)
          `,
          backgroundSize: '40px 40px',
          pointerEvents: 'none',
        }}
      />

      <div
        style={{
          position: 'absolute',
          width: '300px',
          height: '300px',
          borderRadius: '50%',
          background: 'radial-gradient(circle, rgba(102, 126, 234, 0.15) 0%, transparent 70%)',
          top: '10%',
          left: '15%',
          animation: 'float 8s ease-in-out infinite',
          pointerEvents: 'none',
        }}
      />
      <div
        style={{
          position: 'absolute',
          width: '200px',
          height: '200px',
          borderRadius: '50%',
          background: 'radial-gradient(circle, rgba(118, 75, 162, 0.15) 0%, transparent 70%)',
          bottom: '15%',
          right: '20%',
          animation: 'float 6s ease-in-out infinite 2s',
          pointerEvents: 'none',
        }}
      />

      <div style={{ position: 'relative', zIndex: 10, animation: 'slideUp 500ms ease-out' }}>
        <Card
          style={{
            width: 420,
            background: 'var(--bg-card)',
            border: '1px solid var(--border-subtle)',
            borderRadius: 'var(--radius-lg)',
            boxShadow: 'var(--shadow-lg), var(--shadow-glow)',
            backdropFilter: 'blur(20px)',
          }}
          styles={{ body: { padding: '40px 32px' } }}
        >
          <div
            style={{
              width: '64px',
              height: '64px',
              margin: '0 auto 24px',
              borderRadius: 'var(--radius-md)',
              background: 'var(--gradient-primary)',
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              boxShadow: '0 4px 20px rgba(102, 126, 234, 0.3)',
            }}
          >
            <svg
              width="32"
              height="32"
              viewBox="0 0 24 24"
              fill="none"
              stroke="white"
              strokeWidth="2"
              strokeLinecap="round"
              strokeLinejoin="round"
            >
              <path d="M20 21v-2a4 4 0 0 0-4-4H8a4 4 0 0 0-4 4v2" />
              <circle cx="12" cy="7" r="4" />
            </svg>
          </div>

          <Title
            level={3}
            style={{
              textAlign: 'center',
              marginBottom: '8px',
              color: 'var(--text-primary)',
              fontFamily: "'Space Grotesk', sans-serif",
              fontWeight: 600,
              letterSpacing: '-0.5px',
            }}
          >
            用户中心
          </Title>
          <p
            style={{
              textAlign: 'center',
              color: 'var(--text-secondary)',
              marginBottom: '32px',
              fontSize: '14px',
            }}
          >
            User Center Portal
          </p>
          <LoginForm />
        </Card>

        <p
          style={{
            textAlign: 'center',
            color: 'var(--text-muted)',
            fontSize: '12px',
            marginTop: '24px',
          }}
        >
          Hive-Claw User System &copy; 2026
        </p>
      </div>
    </div>
  );
};

export default LoginPage;
