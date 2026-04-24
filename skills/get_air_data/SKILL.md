---
name: get_air_data
description: 当用户查询航空装备、支援保障力量，航材等相关信息
tags: air
---

# 航空装备数据查询 Skill

## 触发条件

用户查询航空装备相关信息，例如：
- 航空飞机
- 航空设备
- 支援保障力量

## 执行步骤

1. **获取数据库列表**
   - 调用 `mcp_list_databases`
   - 选择 `hqjc` 数据库

2. **获取表结构**
   - 调用 `mcp_list_tables`，参数 `dbname: "hqjc"`
   - 根据用户问题选择相关表

3. **查看表详情**
   - 调用 `mcp_describe_table`，参数 `table_name: <表名>`, `dbname: "hqjc"`
   - 确认相关列和数据类型

4. **执行查询**
   - 调用 `mcp_query_database`，参数 `sql: <SQL语句>`, `dbname: "hqjc"`
   - 构建 SQL 时注意：
     - 使用正确的表名和列名
     - 添加必要的 WHERE 条件
     - 使用聚合函数（SUM/COUNT/AVG）处理统计需求

5. **返回结果**
   - 将查询结果以清晰格式返回给用户
   - 必要时添加单位说明（吨、升等）
