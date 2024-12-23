# 第一阶段：构建阶段
FROM rust:bullseye as builder

WORKDIR /app

# 安装构建依赖
RUN apt-get update && apt-get install -y \
    build-essential cmake libssl-dev pkg-config libsqlite3-dev

# 缓存依赖以加快构建速度
RUN mkdir src && echo 'fn main() {}' > src/main.rs
COPY Cargo.toml .
COPY Cargo.lock .
RUN cargo build --target-dir=target --release && rm -f src/main.rs

# 复制代码并执行构建
COPY . .
RUN cargo install --target-dir=target --bin=exloli --path .

# 第二阶段：精简运行环境
FROM debian:bullseye-slim

# 环境变量
ENV RUST_BACKTRACE=full
WORKDIR /app

# 安装运行时依赖
RUN apt-get update \
    && apt-get install -y \
        libsqlite3-0 \
        libssl1.1 \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && rm -rf /var/cache/apt/archives/*

# 配置 OpenSSL 以兼容更低的安全级别（如果需要）
RUN echo "\
[system_default_sect] \n\
MinProtocol = TLSv1.2 \n\
CipherString = DEFAULT@SECLEVEL=1 \n\
" >> /etc/ssl/openssl.cnf

# 复制可执行文件到运行时镜像
COPY --from=builder /usr/local/cargo/bin/exloli /usr/local/bin/exloli

# 默认执行命令
CMD ["exloli"]
