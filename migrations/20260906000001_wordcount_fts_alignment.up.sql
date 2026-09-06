-- 提取 table 分片 cells[].text 的字符数
CREATE OR REPLACE FUNCTION chunk_cells_char_count(content JSONB)
RETURNS INTEGER
LANGUAGE plpgsql
IMMUTABLE PARALLEL SAFE
AS $$
DECLARE
    total INTEGER := 0;
    cell JSONB;
BEGIN
    IF content ? 'cells' THEN
        FOR cell IN SELECT jsonb_array_elements(content->'cells')
        LOOP
            total := total + COALESCE(length(cell->>'text'), 0);
        END LOOP;
    END IF;
    RETURN total;
END;
$$;

COMMENT ON FUNCTION chunk_cells_char_count IS
'统计 table 分片 cells[].text 的字符数，与 chunk_cells_text 配套使用。
注意：word_count 列存储的是"字符数"近似值，非严格"词数"（英文场景差异明显）';
