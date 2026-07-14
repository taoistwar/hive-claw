# Dev 环境

## Redis

本地启动命令、Direct/Sentinel 配置和当前 Redis 用途见
[Redis 开发指南](dev-redis.md)，避免维护两份不一致的端口和密码示例。

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
