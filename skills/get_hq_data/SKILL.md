---
name: get_hq_data
description: 当用户查询后勤维修相关信息时触发，触发词包括：抢修、维修
tags: hqjc
---

# 后勤维修数据查询 Skill

## 触发条件

用户查询油料相关信息，例如：
- 飞机维修
- 抢修
- 突发故障

## 执行步骤

1. **查看表详情**
   - 调用 `mcp_describe_table`，参数 `table_name: hqjc_jzll`, `dbname: "hqjc"`
   - 确认相关列和数据类型

3. **执行查询**
   - 调用 `mcp_vector_search_from_query`，参数 `sql_template: <SQL语句>`, `dbname: "hqjc"`,`query: "<用户查询>"`, `model:qwen3-embedding-8b`
   - 构建 SQL 时注意：
     - 使用正确的表名和列名
     - 添加必要的 WHERE 条件
     - 添加 ORDER BY embedding <=> vector 进行向量查询，向量用占位符表示

     - 参考示例：
       ```sql
       SELECT id, main_task, main_support_capability
         FROM hqjc_jzll
         ORDER BY embedding <=> {embedding}
         LIMIT 5
       ```

4. **返回结果**
   - 将查询结果以清晰格式返回给用户
   - 必要时添加单位说明
