-- 20260801000001_extensions
--
-- 中文全文检索的前置依赖。必须先于 init_schema 执行：
-- document_chunks.content_tsv 与 documents.title_tsv 是 GENERATED ALWAYS 生成列，
-- 建表时就要能解析 to_tsvector('zhparser_cfg', ...)，配置不存在则建表直接失败。
--
-- 镜像要求：官方 postgres:16 不带 zhparser，须使用 docker/postgres-zhparser 构建的镜像。

CREATE EXTENSION IF NOT EXISTS zhparser;

-- zhparser_cfg：把 SCWS 分词结果映射为可检索 token。
-- 映射的词性（见 SCWS 词性表）：
--   n 名词  v 动词  a 形容词  i 成语  e 叹词  l 习用语
--   j 简称  f 方位词  t 时间词  x 未知词（含英文/数字串）
-- 未映射的词性（标点 w、助词 u、语气词 y 等）被当作停用词丢弃。
CREATE TEXT SEARCH CONFIGURATION zhparser_cfg (PARSER = zhparser);

ALTER TEXT SEARCH CONFIGURATION zhparser_cfg
    ADD MAPPING FOR n, v, a, i, e, l, j, f, t, x WITH simple;

-- multi_short：把长词额外切出短词（"知识库系统" 同时产出 "知识"/"库"/"系统"），
-- 提升召回率，代价是 tsvector 体积变大。对文档检索场景收益大于代价。
-- 作用于新建会话，故须在应用连接之前执行本迁移。
DO $$
BEGIN
    EXECUTE format(
        'ALTER DATABASE %I SET zhparser.multi_short = on',
        current_database()
    );
END
$$;
