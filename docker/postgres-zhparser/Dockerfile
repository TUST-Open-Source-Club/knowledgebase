# PostgreSQL 16 + zhparser（中文分词）
#
# 为什么需要自建镜像：
#   数据库设计文档 §8.3 的全文检索方案依赖 zhparser 分词器，
#   而 document_chunks.content_tsv 是 GENERATED ALWAYS 生成列，
#   建表时就必须能解析 to_tsvector('zhparser_cfg', ...)。
#   官方 postgres:16 镜像不带 zhparser，迁移会在第一条就失败。
#
# 构建：
#   docker build -t kb-postgres:16-zhparser docker/postgres-zhparser
# 若在能直连 deb.debian.org 的网络里，可用官方源：
#   docker build --build-arg APT_MIRROR=deb.debian.org ...

FROM postgres:16 AS builder

# 默认走清华镜像：实测直连 deb.debian.org 的 apt-get update 会挂到超时，
# 换镜像后 2 秒完成。改成官方源见文件头注释。
ARG APT_MIRROR=mirrors.tuna.tsinghua.edu.cn
ARG SCWS_VERSION=1.2.3
ARG ZHPARSER_REF=master

RUN set -eux; \
    if [ -f /etc/apt/sources.list.d/debian.sources ]; then \
        sed -i "s|deb.debian.org|${APT_MIRROR}|g" /etc/apt/sources.list.d/debian.sources; \
    fi; \
    if [ -f /etc/apt/sources.list ]; then \
        sed -i "s|deb.debian.org|${APT_MIRROR}|g" /etc/apt/sources.list; \
    fi; \
    apt-get update; \
    apt-get install -y --no-install-recommends \
        build-essential \
        ca-certificates \
        curl \
        git \
        make \
        postgresql-server-dev-16; \
    rm -rf /var/lib/apt/lists/*

# SCWS：zhparser 依赖的分词引擎。
# 用官方发布 tarball 而非 git 仓库：仓库里没有 configure，
# 而 autoreconf 引导会因 automake 版本不兼容失败。
WORKDIR /tmp/build
RUN set -eux; \
    curl -sSL -o scws.tar.bz2 \
        "http://www.xunsearch.com/scws/down/scws-${SCWS_VERSION}.tar.bz2"; \
    tar xjf scws.tar.bz2; \
    cd "scws-${SCWS_VERSION}"; \
    ./configure --prefix=/usr/local; \
    make -j"$(nproc)"; \
    make install; \
    ls -l /usr/local/lib/libscws.so.1

# zhparser：把 SCWS 包装成 PostgreSQL 分词器扩展
RUN set -eux; \
    git clone --depth 1 --branch "${ZHPARSER_REF}" https://github.com/amutu/zhparser.git zhparser; \
    cd zhparser; \
    SCWS_HOME=/usr/local make -j"$(nproc)"; \
    make install; \
    ls -l /usr/lib/postgresql/16/lib/zhparser.so


FROM postgres:16

# 只搬运产物，不把 build-essential 留在运行镜像里。
# tsearch_data 里是 zhparser 装的词典与规则（dict.utf8.xdb / rules.utf8.ini），
# 缺了它 CREATE EXTENSION 能成功但分词结果为空。
COPY --from=builder /usr/local/lib/libscws.so* /usr/local/lib/
COPY --from=builder /usr/lib/postgresql/16/lib/zhparser.so /usr/lib/postgresql/16/lib/
COPY --from=builder /usr/share/postgresql/16/extension/zhparser* /usr/share/postgresql/16/extension/
COPY --from=builder /usr/share/postgresql/16/tsearch_data/ /usr/share/postgresql/16/tsearch_data/

RUN set -eux; \
    echo /usr/local/lib > /etc/ld.so.conf.d/scws.conf; \
    ldconfig

# 数据库设计文档 §1.5：字符集 UTF8 / 排序规则 C.UTF-8（不依赖系统 locale）
ENV POSTGRES_INITDB_ARGS="--encoding=UTF8 --lc-collate=C.UTF-8 --lc-ctype=C.UTF-8"
