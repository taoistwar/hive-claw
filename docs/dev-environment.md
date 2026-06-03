# Dev 环境

## Redis

```bash
docker run -d --restart=always --name agent_redis_16379 -p 16379:6379 redis redis-server --requirepass "ai123456"
```

## Rustfs

```bash
git clone https://ghfast.top/https://github.com/rustfs/rustfs
cd rustfs
sudo mkdir -p /opt1/rustfs /opt2/rustfs /opt3/rustfs /opt4/rustfs
sudo chown -R 10001:10001 /opt1/rustfs /opt2/rustfs /opt3/rustfs /opt4/rustfs
sudo chmod -R 755 /opt1/rustfs /opt2/rustfs /opt3/rustfs /opt4/rustfs
docker compose --profile observability up -d
```

## MySQL

```bash
docker run -d --restart=always --name agent_mysql_stack -p 33060:3306 -e MYSQL_ROOT_PASSWORD='ai123456' mysql:5.7.41
```
