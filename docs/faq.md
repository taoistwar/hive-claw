# 常见问题

## 数据库连接失败

确保 MySQL 正在运行并且数据库已创建：

```bash
mysql -u root -p -e "SHOW DATABASES LIKE 'hiveweb';"
```

## Redis 连接失败

确保 Redis 正在运行：

```bash
redis-cli ping
# 应该返回 PONG
```

## 端口被占用

修改端口：

```bash
export HIVEWEB_PORT=3001
cargo run
```
