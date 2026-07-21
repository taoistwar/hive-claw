# 部署

## 生产环境构建

```bash
# 后端，使用 musl 进行全静态编译。（musl 是一个轻量级的 C 标准库，支持完全静态链接，生成的可执行文件不依赖目标系统的任何动态库。）
rustup target add x86_64-unknown-linux-musl
cargo build -p hiveweb --release  --target x86_64-unknown-linux-musl

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
