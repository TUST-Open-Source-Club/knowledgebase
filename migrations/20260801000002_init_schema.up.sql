-- 20260801000002_init_schema
--
-- 八张表的 baseline。建表顺序遵循外键依赖拓扑：
--   users → spaces → media_assets → documents → document_chunks
--         → space_members → document_permissions → refresh_tokens
--
-- 索引统一放在 20260801000003_indexes，本文件只建表、约束与触发器。

-- ============================================================
-- updated_at 自动维护
-- ============================================================

CREATE OR REPLACE FUNCTION trg_set_updated_at()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = now();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- 抽取 table chunk 的 cells 文本供全文检索。
--
-- 为什么不用子查询 / jsonb_array_elements：生成列表达式禁用子查询，
-- 也禁用集合返回函数（SRF），二者都会让 CREATE TABLE 直接失败。
-- 故用 -> 下标遍历的 plpgsql 循环。
--
-- IMMUTABLE 是真话而非谎标：纯 jsonb 操作，不读表、不碰时钟/locale，
-- 同一输入永远产出同一输出——满足生成列对 IMMUTABLE 的实质要求。
-- 代价：plpgsql 逐元素循环比 SRF 慢，但 cells 规模很小（一张表几十个格），
-- 且只在写入时求值一次，可忽略。
CREATE OR REPLACE FUNCTION chunk_cells_text(content jsonb)
RETURNS text
LANGUAGE plpgsql
IMMUTABLE PARALLEL SAFE
AS $fn$
DECLARE
    cells jsonb;
    n     int;
    i     int;
    acc   text := '';
BEGIN
    cells := content -> 'cells';
    IF cells IS NULL OR jsonb_typeof(cells) <> 'array' THEN
        RETURN '';
    END IF;
    n := jsonb_array_length(cells);
    FOR i IN 0 .. n - 1 LOOP
        acc := acc || coalesce(cells -> i ->> 'text', '') || ' ';
    END LOOP;
    RETURN acc;
END;
$fn$;

-- ============================================================
-- 1. users
-- ============================================================

CREATE TABLE users (
    id            UUID         PRIMARY KEY DEFAULT gen_random_uuid(), -- 应用层生成 UUID v7
    email         VARCHAR(256) NOT NULL,   -- 唯一性由部分索引保证，见 indexes 迁移
    password_hash VARCHAR(256) NOT NULL,   -- argon2 哈希
    nickname      VARCHAR(128),
    avatar        VARCHAR(512),
    role          VARCHAR(20)  NOT NULL DEFAULT 'user',   -- admin / user
    status        VARCHAR(20)  NOT NULL DEFAULT 'active', -- active / banned
    created_at    TIMESTAMPTZ  NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ  NOT NULL DEFAULT now(),
    deleted_at    TIMESTAMPTZ,                            -- 软删除
    CONSTRAINT chk_users_role   CHECK (role   IN ('admin','user')),
    CONSTRAINT chk_users_status CHECK (status IN ('active','banned')),
    -- 公开权限哨兵 UUID 不得成为真实用户，否则 document_permissions 的
    -- "公开" 语义与 "某个具体用户" 语义会混淆。
    CONSTRAINT chk_users_id_not_public_sentinel
        CHECK (id <> '00000000-0000-0000-0000-000000000000')
);

CREATE TRIGGER set_updated_at BEFORE UPDATE ON users
    FOR EACH ROW EXECUTE FUNCTION trg_set_updated_at();

-- ============================================================
-- 2. spaces
-- ============================================================

CREATE TABLE spaces (
    id                UUID         PRIMARY KEY DEFAULT gen_random_uuid(),
    name              VARCHAR(128) NOT NULL,
    slug              VARCHAR(128) NOT NULL, -- 唯一性由部分索引保证
    owner_id          UUID         NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    description       TEXT,
    visibility        VARCHAR(20)  NOT NULL DEFAULT 'private', -- private / team / public
    created_at        TIMESTAMPTZ  NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ  NOT NULL DEFAULT now(),
    deleted_at        TIMESTAMPTZ,
    -- 级联软删批次标记：软删空间时为本次操作生成一个批次 ID，
    -- 同时写入该空间行与被连带软删的 documents 行。
    -- 恢复时据此只恢复"跟着空间一起被删"的文档，
    -- 先前已被单独软删的文档保持删除状态。单独软删时为 NULL。
    -- 无外键：纯批次标记，不引用任何表。
    deleted_batch_id  UUID,
    CONSTRAINT chk_spaces_visibility CHECK (visibility IN ('private','team','public')),
    CONSTRAINT chk_spaces_batch_requires_deleted
        CHECK (deleted_batch_id IS NULL OR deleted_at IS NOT NULL)
);

CREATE TRIGGER set_updated_at BEFORE UPDATE ON spaces
    FOR EACH ROW EXECUTE FUNCTION trg_set_updated_at();

-- ============================================================
-- 3. media_assets
-- ============================================================

CREATE TABLE media_assets (
    id           UUID         PRIMARY KEY DEFAULT gen_random_uuid(),
    uploader_id  UUID         REFERENCES users(id) ON DELETE SET NULL,
    type         VARCHAR(20)  NOT NULL,  -- image / video / file，由 Rust enum 校验
    filename     VARCHAR(256) NOT NULL,
    mime_type    VARCHAR(128),
    size_bytes   BIGINT       NOT NULL DEFAULT 0,
    storage_path VARCHAR(512) NOT NULL,  -- MinIO bucket 内相对路径
    url          VARCHAR(512),
    metadata     JSONB        NOT NULL DEFAULT '{}',
    created_at   TIMESTAMPTZ  NOT NULL DEFAULT now(),
    deleted_at   TIMESTAMPTZ,            -- 孤儿清理标记（非用户可见回收站）
    CONSTRAINT chk_media_assets_size_bytes CHECK (size_bytes >= 0)
    -- 无 updated_at：上传后元数据不变，故不挂 set_updated_at 触发器
);

-- ============================================================
-- 4. documents
-- ============================================================

CREATE TABLE documents (
    id          UUID         PRIMARY KEY DEFAULT gen_random_uuid(),
    space_id    UUID         NOT NULL REFERENCES spaces(id) ON DELETE RESTRICT,
    title       VARCHAR(256) NOT NULL,
    slug        VARCHAR(256),
    author_id   UUID         REFERENCES users(id) ON DELETE SET NULL,
    status      VARCHAR(20)  NOT NULL DEFAULT 'draft', -- draft / published / archived
    cover_image VARCHAR(512),                          -- 逻辑引用 media_assets.url
    word_count  INTEGER      NOT NULL DEFAULT 0,       -- 由 chunk 写入事务末尾回写
    parent_id   UUID         REFERENCES documents(id) ON DELETE SET NULL,
    sort_order  INTEGER      NOT NULL DEFAULT 0,
    created_at  TIMESTAMPTZ  NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ  NOT NULL DEFAULT now(),
    deleted_at  TIMESTAMPTZ,
    deleted_batch_id UUID,   -- 见 spaces.deleted_batch_id
    -- 标题检索：与 chunk 正文分开索引，搜索时两路 UNION 后按 document_id 聚合。
    -- 生成列由 PostgreSQL 保证与 title 永不失同步，无需同步 worker。
    title_tsv   tsvector GENERATED ALWAYS AS (
                    to_tsvector('zhparser_cfg', coalesce(title, ''))
                ) STORED,
    CONSTRAINT chk_documents_status CHECK (status IN ('draft','published','archived')),
    CONSTRAINT chk_documents_word_count CHECK (word_count >= 0),
    CONSTRAINT chk_documents_sort_order CHECK (sort_order >= 0),
    -- 自引用（parent_id = id）由数据库拦截；
    -- 深层循环（A→B→C→A）与跨空间父节点仍由应用层拦截。
    CONSTRAINT chk_documents_no_self_parent CHECK (parent_id <> id),
    CONSTRAINT chk_documents_batch_requires_deleted
        CHECK (deleted_batch_id IS NULL OR deleted_at IS NOT NULL)
);

CREATE TRIGGER set_updated_at BEFORE UPDATE ON documents
    FOR EACH ROW EXECUTE FUNCTION trg_set_updated_at();

-- ============================================================
-- 5. document_chunks
-- ============================================================

CREATE TABLE document_chunks (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    document_id UUID        NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    type        VARCHAR(32) NOT NULL,   -- 不加 CHECK；由 Rust enum 反序列化校验
    content     JSONB       NOT NULL,
    position    INTEGER     NOT NULL,   -- 文档内排序，从 0 开始
    version     INTEGER     NOT NULL DEFAULT 1, -- 乐观锁，一期预留
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- 正文检索：只抽 content 中人类可读的字段，不用 content::text
    -- （否则 asset_id、宽高、language 等元数据会被当成检索词）。
    -- 覆盖的 chunk 类型字段：
    --   text    → paragraph / heading_* / blockquote / list_item / callout
    --   code    → code_block
    --   caption → image / video / file
    --   name    → file
    --   url     → embed
    --   cells[].text → table（经 chunk_cells_text 抽取，见本迁移顶部）
    content_tsv tsvector GENERATED ALWAYS AS (
                    to_tsvector('zhparser_cfg',
                        coalesce(content->>'text',    '') || ' ' ||
                        coalesce(content->>'code',    '') || ' ' ||
                        coalesce(content->>'caption', '') || ' ' ||
                        coalesce(content->>'name',    '') || ' ' ||
                        coalesce(content->>'url',     '') || ' ' ||
                        chunk_cells_text(content)
                    )
                ) STORED,
    CONSTRAINT chk_chunks_position CHECK (position >= 0),
    CONSTRAINT chk_chunks_version  CHECK (version >= 1),
    -- DEFERRABLE INITIALLY IMMEDIATE：
    --   默认即时校验（普通写入的错误尽早暴露）；
    --   reorder 事务开头执行
    --     SET CONSTRAINTS uniq_chunks_doc_position DEFERRED
    --   即可在事务内出现临时重复 position，COMMIT 时才整体校验。
    --   代价：DEFERRABLE 约束不能用于 ON CONFLICT，
    --   故 chunk upsert 必须按主键 id 冲突，不得按 (document_id, position) 冲突。
    CONSTRAINT uniq_chunks_doc_position UNIQUE (document_id, position)
        DEFERRABLE INITIALLY IMMEDIATE
    -- 无 deleted_at：分片随文档 CASCADE 物理删除
);

CREATE TRIGGER set_updated_at BEFORE UPDATE ON document_chunks
    FOR EACH ROW EXECUTE FUNCTION trg_set_updated_at();

-- ============================================================
-- 6. space_members
-- ============================================================

CREATE TABLE space_members (
    space_id  UUID        NOT NULL REFERENCES spaces(id) ON DELETE CASCADE,
    user_id   UUID        NOT NULL REFERENCES users(id)  ON DELETE CASCADE,
    role      VARCHAR(20) NOT NULL DEFAULT 'viewer', -- owner / editor / viewer
    joined_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (space_id, user_id),
    CONSTRAINT chk_space_members_role CHECK (role IN ('owner','editor','viewer'))
    -- 每空间至多一个 owner：由 indexes 迁移中的部分唯一索引保证
);

-- ============================================================
-- 7. document_permissions
-- ============================================================

CREATE TABLE document_permissions (
    document_id UUID        NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    -- 无外键：哨兵 UUID 00000000-0000-0000-0000-000000000000 表示"所有用户（公开）"，
    -- 它不存在于 users 表中（由 chk_users_id_not_public_sentinel 保证）。
    -- 该哨兵仅允许出现在本列。
    user_id     UUID        NOT NULL,
    permission  VARCHAR(20) NOT NULL, -- read / write / admin / none
    granted_by  UUID        REFERENCES users(id) ON DELETE SET NULL,
    granted_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (document_id, user_id),
    CONSTRAINT chk_doc_permissions_perm CHECK (permission IN ('read','write','admin','none')),
    -- 授权人必须是真实用户（或注销后的 NULL），不能是公开哨兵
    CONSTRAINT chk_doc_permissions_granted_by_not_public
        CHECK (granted_by IS NULL OR granted_by <> '00000000-0000-0000-0000-000000000000')
);

-- ============================================================
-- 8. refresh_tokens
-- ============================================================

CREATE TABLE refresh_tokens (
    id          UUID         PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id     UUID         NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_hash  VARCHAR(128) NOT NULL UNIQUE, -- SHA-256(refresh_token)，绝不存明文
    device_info VARCHAR(256),
    ip          INET,
    expires_at  TIMESTAMPTZ  NOT NULL,
    revoked_at  TIMESTAMPTZ,                  -- NULL = 有效
    created_at  TIMESTAMPTZ  NOT NULL DEFAULT now()
    -- 无 updated_at / deleted_at：过期或注销后由定时任务物理删除
);
