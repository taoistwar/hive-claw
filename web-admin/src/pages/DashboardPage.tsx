import Dashboard from '../components/Dashboard';
import { RuntimePoolCard } from '../components/RuntimePoolCard';

const DashboardPage: React.FC = () => {
  return (
    <div>
      <Dashboard />
      <RuntimePoolCard />
    </div>
  );
};

export default DashboardPage;
