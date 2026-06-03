# 部署

## 生产环境构建

```bash
# 后端
cd crates/hiveweb
cargo build --release

# 前端
cd web-admin
npm run build
```

## Docker 部署（可选）

```bash
# 构建镜像
docker build -t hiveweb .

# 运行容器
docker run -p 3300:3300 hiveweb
```
